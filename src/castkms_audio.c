// SPDX-License-Identifier: GPL-2.0+

#include <linux/anon_inodes.h>
#include <linux/device/faux.h>
#include <linux/file.h>
#include <linux/fs.h>
#include <linux/hrtimer.h>
#include <linux/kfifo.h>
#include <linux/kref.h>
#include <linux/math64.h>
#include <linux/mutex.h>
#include <linux/poll.h>
#include <linux/slab.h>
#include <linux/spinlock.h>
#include <linux/string.h>
#include <linux/uaccess.h>
#include <linux/wait.h>

#include <drm/drm_connector.h>
#include <drm/drm_drv.h>
#include <drm/drm_edid.h>
#include <drm/drm_eld.h>
#include <drm/drm_print.h>

#include <sound/control.h>
#include <sound/core.h>
#include <sound/info.h>
#include <sound/jack.h>
#include <sound/pcm.h>
#include <sound/pcm_drm_eld.h>
#include <sound/tlv.h>

#include "castkms_audio.h"
#include "castkms_capture_authority.h"
#include "castkms_config.h"
#include "castkms_connector.h"
#include "castkms_device.h"
#include "castkms_display_identity.h"

#define CASTKMS_AUDIO_TAP_RATE		48000U
#define CASTKMS_AUDIO_TAP_CHANNELS	2U
#define CASTKMS_AUDIO_TAP_SAMPLE_BYTES	2U
#define CASTKMS_AUDIO_TAP_FRAME_BYTES \
	(CASTKMS_AUDIO_TAP_CHANNELS * CASTKMS_AUDIO_TAP_SAMPLE_BYTES)
#define CASTKMS_AUDIO_TAP_PERIOD_FRAMES	480U
#define CASTKMS_AUDIO_TAP_PERIOD_BYTES \
	(CASTKMS_AUDIO_TAP_PERIOD_FRAMES * CASTKMS_AUDIO_TAP_FRAME_BYTES)
/* kfifo capacity is a power of two: about 1.36 seconds at 48 kHz. */
#define CASTKMS_AUDIO_TAP_BUFFER_BYTES	(1U << 18)
#define CASTKMS_AUDIO_TAP_BUFFER_FRAMES \
	(CASTKMS_AUDIO_TAP_BUFFER_BYTES / CASTKMS_AUDIO_TAP_FRAME_BYTES)

struct castkms_audio_tap;
struct castkms_audio_runtime;

struct castkms_audio_card {
	struct kref ref;
	struct snd_card *card;
	struct snd_jack *jack;
	struct snd_kcontrol *eld_ctl;
	char display_name[80];

	struct mutex lock;
	spinlock_t stream_lock;
	u8 eld[MAX_ELD_BYTES];
	unsigned int generation;
	struct snd_pcm_substream *active_substream;
	struct castkms_audio_runtime *playback_runtime;
	u64 playback_generation;
	struct castkms_audio_tap *tap;
};

struct castkms_audio_slot {
	struct drm_connector *connector;
	int index;
	struct castkms_audio_card *card;
};

struct castkms_audio {
	struct mutex lock;
	unsigned int num_outputs;
	struct castkms_audio_slot *outputs;
};

struct castkms_audio_runtime {
	struct snd_pcm_substream *substream;
	struct hrtimer timer;
	spinlock_t clock_lock;
	ktime_t period_duration;
	ktime_t base_time;
	u64 base_frames;
	unsigned int eld_generation;
	uint8_t eld[MAX_ELD_BYTES];
	bool running;
};

struct castkms_audio_tap {
	struct castkms_audio_card *output;
	struct castkms_capture_authority *authority;
	struct castkms_capture_authority_resource authority_resource;
	struct hrtimer timer;
	ktime_t period_duration;
	ktime_t base_time;
	u64 produced_frames;
	u64 playback_generation;
	u64 playback_frames;
	struct kfifo fifo;
	spinlock_t fifo_lock;
	struct mutex read_lock;
	wait_queue_head_t wait;
	int terminal_status;
	u64 dropped_frames;
	u8 *read_scratch;
	u8 scratch[CASTKMS_AUDIO_TAP_PERIOD_BYTES];
};

static struct castkms_audio_slot *
castkms_audio_find_slot(struct castkms_audio *audio,
			struct drm_connector *connector);

static const struct snd_pcm_hardware castkms_pcm_hw = {
	.info = SNDRV_PCM_INFO_INTERLEAVED |
		SNDRV_PCM_INFO_MMAP |
		SNDRV_PCM_INFO_MMAP_VALID |
		SNDRV_PCM_INFO_PAUSE |
		SNDRV_PCM_INFO_HAS_LINK_ESTIMATED_ATIME,
	.formats = SNDRV_PCM_FMTBIT_S16_LE,
	.rates = SNDRV_PCM_RATE_48000,
	.rate_min = 48000,
	.rate_max = 48000,
	.channels_min = 2,
	.channels_max = 2,
	.period_bytes_min = 64 * 2 * 2,
	.period_bytes_max = 16384 * 2 * 2,
	.periods_min = 2,
	.periods_max = 32,
	.buffer_bytes_max = 65536 * 2 * 2,
};

static void castkms_audio_card_release(struct kref *ref)
{
	struct castkms_audio_card *output =
		container_of(ref, struct castkms_audio_card, ref);

	mutex_destroy(&output->lock);
	kfree(output);
}

static void castkms_audio_card_get(struct castkms_audio_card *output)
{
	kref_get(&output->ref);
}

static void castkms_audio_card_put(struct castkms_audio_card *output)
{
	kref_put(&output->ref, castkms_audio_card_release);
}

static void castkms_audio_snd_card_free(struct snd_card *card)
{
	struct castkms_audio_card *output = card->private_data;

	card->private_data = NULL;
	mutex_lock(&output->lock);
	output->card = NULL;
	mutex_unlock(&output->lock);
	castkms_audio_card_put(output);
}

/* --- frame position --- */

static u64 castkms_audio_frames_since_base(struct castkms_audio_runtime *rt,
					    ktime_t now, unsigned int rate)
{
	u64 elapsed_ns;

	elapsed_ns = ktime_to_ns(ktime_sub(now, rt->base_time));
	return mul_u64_u32_div(elapsed_ns, rate, NSEC_PER_SEC);
}

static u64 castkms_audio_total_frames_locked(struct castkms_audio_runtime *rt,
					      ktime_t now,
					      unsigned int rate)
{
	if (!rt->running)
		return rt->base_frames;

	return rt->base_frames +
	       castkms_audio_frames_since_base(rt, now, rate);
}

static u64 castkms_audio_total_frames(struct castkms_audio_runtime *rt,
				       ktime_t now, unsigned int rate)
{
	unsigned long flags;
	u64 frames;

	spin_lock_irqsave(&rt->clock_lock, flags);
	frames = castkms_audio_total_frames_locked(rt, now, rate);
	spin_unlock_irqrestore(&rt->clock_lock, flags);

	return frames;
}

static bool castkms_audio_runtime_is_running(
	struct castkms_audio_runtime *rt)
{
	unsigned long flags;
	bool running;

	spin_lock_irqsave(&rt->clock_lock, flags);
	running = rt->running;
	spin_unlock_irqrestore(&rt->clock_lock, flags);

	return running;
}

/* --- hrtimer --- */

static enum hrtimer_restart castkms_audio_timer(struct hrtimer *timer)
{
	struct castkms_audio_runtime *rt =
		container_of(timer, struct castkms_audio_runtime, timer);
	ktime_t period_duration;
	unsigned long flags;
	bool running;

	spin_lock_irqsave(&rt->clock_lock, flags);
	running = rt->running;
	period_duration = rt->period_duration;
	spin_unlock_irqrestore(&rt->clock_lock, flags);
	if (!running)
		return HRTIMER_NORESTART;

	hrtimer_forward_now(timer, period_duration);
	snd_pcm_period_elapsed(rt->substream);

	if (!castkms_audio_runtime_is_running(rt))
		return HRTIMER_NORESTART;
	return HRTIMER_RESTART;
}

static void castkms_audio_timer_arm(struct castkms_audio_runtime *rt)
{
	unsigned long flags;
	ktime_t period_duration;

	spin_lock_irqsave(&rt->clock_lock, flags);
	rt->running = true;
	rt->base_time = ktime_get();
	period_duration = rt->period_duration;
	spin_unlock_irqrestore(&rt->clock_lock, flags);
	hrtimer_start(&rt->timer, period_duration, HRTIMER_MODE_REL_SOFT);
}

static void castkms_audio_timer_disarm(struct castkms_audio_runtime *rt,
				       unsigned int rate)
{
	ktime_t now = ktime_get();
	unsigned long flags;

	spin_lock_irqsave(&rt->clock_lock, flags);
	rt->base_frames = castkms_audio_total_frames_locked(rt, now, rate);
	rt->running = false;
	spin_unlock_irqrestore(&rt->clock_lock, flags);
	hrtimer_try_to_cancel(&rt->timer);
}

/* --- PCM callbacks --- */

static int castkms_pcm_open(struct snd_pcm_substream *substream)
{
	struct castkms_audio_card *out = substream->private_data;
	struct castkms_audio_runtime *rt;
	int ret;

	mutex_lock(&out->lock);
	if (out->active_substream) {
		mutex_unlock(&out->lock);
		return -EBUSY;
	}

	rt = kzalloc(sizeof(*rt), GFP_KERNEL);
	if (!rt) {
		mutex_unlock(&out->lock);
		return -ENOMEM;
	}

	rt->substream = substream;
	rt->eld_generation = out->generation;
	memcpy(rt->eld, out->eld, sizeof(rt->eld));
	spin_lock_init(&rt->clock_lock);
	hrtimer_setup(&rt->timer, castkms_audio_timer,
		      CLOCK_MONOTONIC, HRTIMER_MODE_REL_SOFT);

	substream->runtime->hw = castkms_pcm_hw;
	substream->runtime->private_data = rt;

	/* Constrain parameters to the monitor's capabilities when the
	 * ELD contains Short Audio Descriptors; fall back to the
	 * static hardware descriptor otherwise.
	 */
	ret = snd_pcm_hw_constraint_eld(substream->runtime, rt->eld);
	if (ret) {
		substream->runtime->private_data = NULL;
		kfree(rt);
		mutex_unlock(&out->lock);
		return ret;
	}

	out->active_substream = substream;
	mutex_unlock(&out->lock);

	return 0;
}

static int castkms_pcm_close(struct snd_pcm_substream *substream)
{
	struct castkms_audio_card *out = substream->private_data;
	struct castkms_audio_runtime *rt = substream->runtime->private_data;

	if (castkms_audio_runtime_is_running(rt))
		castkms_audio_timer_disarm(rt, substream->runtime->rate);
	hrtimer_cancel(&rt->timer);

	mutex_lock(&out->lock);
	spin_lock_irq(&out->stream_lock);
	if (out->playback_runtime == rt) {
		out->playback_runtime = NULL;
		out->playback_generation++;
	}
	spin_unlock_irq(&out->stream_lock);
	out->active_substream = NULL;
	mutex_unlock(&out->lock);

	kfree(rt);
	return 0;
}

static int castkms_pcm_hw_params(struct snd_pcm_substream *substream,
				 struct snd_pcm_hw_params *params)
{
	struct castkms_audio_card *out = substream->private_data;
	struct castkms_audio_runtime *rt = substream->runtime->private_data;
	int ret = 0;

	mutex_lock(&out->lock);
	if (rt->eld_generation != out->generation)
		ret = -ENODEV;
	mutex_unlock(&out->lock);

	return ret;
}

static int castkms_pcm_hw_free(struct snd_pcm_substream *substream)
{
	struct castkms_audio_card *out = substream->private_data;
	struct castkms_audio_runtime *rt = substream->runtime->private_data;

	/*
	 * ALSA calls the driver's hw_free callback before releasing a managed
	 * playback buffer. Synchronize with the tap renderer so it cannot retain or
	 * copy from that buffer after this callback returns.
	 */
	spin_lock_irq(&out->stream_lock);
	if (out->playback_runtime == rt) {
		out->playback_runtime = NULL;
		out->playback_generation++;
	}
	spin_unlock_irq(&out->stream_lock);

	return 0;
}

static int castkms_pcm_prepare(struct snd_pcm_substream *substream)
{
	struct castkms_audio_card *out = substream->private_data;
	struct castkms_audio_runtime *rt = substream->runtime->private_data;
	struct snd_pcm_runtime *runtime = substream->runtime;
	u64 period_ns;
	int ret = 0;

	mutex_lock(&out->lock);
	if (rt->eld_generation != out->generation)
		ret = -ENODEV;
	mutex_unlock(&out->lock);

	if (ret)
		return ret;

	period_ns = div_u64((u64)runtime->period_size * NSEC_PER_SEC,
			    runtime->rate);
	spin_lock_irq(&rt->clock_lock);
	rt->period_duration = ns_to_ktime(period_ns);
	rt->base_frames = 0;
	rt->base_time = ktime_set(0, 0);
	spin_unlock_irq(&rt->clock_lock);

	/* The managed playback buffer is ready before prepare runs. */
	spin_lock_irq(&out->stream_lock);
	out->playback_runtime = rt;
	out->playback_generation++;
	spin_unlock_irq(&out->stream_lock);

	return 0;
}

static int castkms_pcm_trigger(struct snd_pcm_substream *substream, int cmd)
{
	struct castkms_audio_runtime *rt = substream->runtime->private_data;
	unsigned int rate = substream->runtime->rate;

	switch (cmd) {
	case SNDRV_PCM_TRIGGER_START:
		spin_lock_irq(&rt->clock_lock);
		rt->base_frames = 0;
		spin_unlock_irq(&rt->clock_lock);
		castkms_audio_timer_arm(rt);
		return 0;
	case SNDRV_PCM_TRIGGER_RESUME:
	case SNDRV_PCM_TRIGGER_PAUSE_RELEASE:
		castkms_audio_timer_arm(rt);
		return 0;
	case SNDRV_PCM_TRIGGER_STOP:
	case SNDRV_PCM_TRIGGER_SUSPEND:
	case SNDRV_PCM_TRIGGER_PAUSE_PUSH:
		castkms_audio_timer_disarm(rt, rate);
		return 0;
	default:
		return -EINVAL;
	}
}

static snd_pcm_uframes_t castkms_pcm_pointer(struct snd_pcm_substream *substream)
{
	struct castkms_audio_runtime *rt = substream->runtime->private_data;
	struct snd_pcm_runtime *runtime = substream->runtime;
	u64 frames;

	frames = castkms_audio_total_frames(rt, ktime_get(), runtime->rate);

	return frames % runtime->buffer_size;
}

static int castkms_pcm_get_time_info(struct snd_pcm_substream *substream,
				     struct timespec64 *system_ts,
				     struct timespec64 *audio_ts,
				     struct snd_pcm_audio_tstamp_config *audio_tstamp_config,
				     struct snd_pcm_audio_tstamp_report *audio_tstamp_report)
{
	struct castkms_audio_runtime *rt = substream->runtime->private_data;
	struct snd_pcm_runtime *runtime = substream->runtime;
	ktime_t now = ktime_get();
	u64 frames;

	if (audio_tstamp_config->type_requested !=
	    SNDRV_PCM_AUDIO_TSTAMP_TYPE_LINK_ESTIMATED) {
		audio_tstamp_report->actual_type =
			SNDRV_PCM_AUDIO_TSTAMP_TYPE_DEFAULT;
		return 0;
	}

	*system_ts = ktime_to_timespec64(now);

	frames = castkms_audio_total_frames(rt, now, runtime->rate);
	*audio_ts = ns_to_timespec64(
		mul_u64_u32_div(frames, NSEC_PER_SEC, runtime->rate));

	audio_tstamp_report->valid = 1;
	audio_tstamp_report->actual_type =
		SNDRV_PCM_AUDIO_TSTAMP_TYPE_LINK_ESTIMATED;
	audio_tstamp_report->accuracy_report = 0;

	return 0;
}

static const struct snd_pcm_ops castkms_pcm_ops = {
	.open = castkms_pcm_open,
	.close = castkms_pcm_close,
	.hw_params = castkms_pcm_hw_params,
	.hw_free = castkms_pcm_hw_free,
	.prepare = castkms_pcm_prepare,
	.trigger = castkms_pcm_trigger,
	.pointer = castkms_pcm_pointer,
	.get_time_info = castkms_pcm_get_time_info,
};

/* --- Grant-scoped playback tap --- */

static bool castkms_audio_tap_is_terminal(struct castkms_audio_tap *tap)
{
	unsigned long flags;
	bool terminal;

	spin_lock_irqsave(&tap->fifo_lock, flags);
	terminal = tap->terminal_status != 0;
	spin_unlock_irqrestore(&tap->fifo_lock, flags);

	return terminal;
}

static void castkms_audio_tap_terminate(struct castkms_audio_tap *tap,
					int status)
{
	unsigned long flags;

	if (status >= 0)
		status = -EIO;
	spin_lock_irqsave(&tap->fifo_lock, flags);
	if (!tap->terminal_status) {
		tap->terminal_status = status;
		tap->dropped_frames +=
			kfifo_len(&tap->fifo) / CASTKMS_AUDIO_TAP_FRAME_BYTES;
		kfifo_reset(&tap->fifo);
	}
	spin_unlock_irqrestore(&tap->fifo_lock, flags);
	wake_up_interruptible_poll(&tap->wait, EPOLLHUP | EPOLLERR);
}

static void castkms_audio_tap_terminate_output_locked(
	struct castkms_audio_card *output, int status)
{
	struct castkms_audio_tap *tap = output->tap;

	lockdep_assert_held(&output->lock);
	if (!tap)
		return;

	output->tap = NULL;
	castkms_audio_tap_terminate(tap, status);
}

static void castkms_audio_tap_append(struct castkms_audio_tap *tap,
				     const u8 *samples, unsigned int bytes)
{
	unsigned long flags;
	unsigned int discarded;

	if (!bytes)
		return;

	spin_lock_irqsave(&tap->fifo_lock, flags);
	if (tap->terminal_status)
		goto out_unlock;
	if (kfifo_avail(&tap->fifo) < bytes) {
		discarded = kfifo_len(&tap->fifo);
		tap->dropped_frames +=
			discarded / CASTKMS_AUDIO_TAP_FRAME_BYTES;
		kfifo_reset(&tap->fifo);
	}
	if (WARN_ON(kfifo_in(&tap->fifo, samples, bytes) != bytes))
		tap->dropped_frames += bytes / CASTKMS_AUDIO_TAP_FRAME_BYTES;

out_unlock:
	spin_unlock_irqrestore(&tap->fifo_lock, flags);
	wake_up_interruptible_poll(&tap->wait, EPOLLIN | EPOLLRDNORM);
}

static void castkms_audio_tap_append_silence(struct castkms_audio_tap *tap,
					     u64 frames)
{
	while (frames) {
		unsigned int chunk = min_t(u64, frames,
					   CASTKMS_AUDIO_TAP_PERIOD_FRAMES);

		memset(tap->scratch, 0,
		       chunk * CASTKMS_AUDIO_TAP_FRAME_BYTES);
		castkms_audio_tap_append(
			tap, tap->scratch,
			chunk * CASTKMS_AUDIO_TAP_FRAME_BYTES);
		frames -= chunk;
	}
}

static void castkms_audio_tap_append_playback(
	struct castkms_audio_tap *tap, struct snd_pcm_runtime *runtime,
	u64 first_frame, u64 frames)
{
	while (frames) {
		snd_pcm_uframes_t offset = first_frame % runtime->buffer_size;
		unsigned int chunk = min_t(u64, frames,
					   CASTKMS_AUDIO_TAP_PERIOD_FRAMES);

		chunk = min_t(snd_pcm_uframes_t, chunk,
			      runtime->buffer_size - offset);
		memcpy(tap->scratch,
		       runtime->dma_area +
			       offset * CASTKMS_AUDIO_TAP_FRAME_BYTES,
		       chunk * CASTKMS_AUDIO_TAP_FRAME_BYTES);
		castkms_audio_tap_append(
			tap, tap->scratch,
			chunk * CASTKMS_AUDIO_TAP_FRAME_BYTES);
		first_frame += chunk;
		frames -= chunk;
	}
}

static void castkms_audio_tap_render(struct castkms_audio_tap *tap,
				     ktime_t now, u64 frames)
{
	struct castkms_audio_card *output = tap->output;
	struct castkms_audio_runtime *playback;
	struct snd_pcm_runtime *runtime = NULL;
	u64 available = 0;
	u64 copy_frames = 0;
	u64 first_frame = 0;
	u64 leading_silence = frames;
	u64 trailing_silence = 0;
	u64 total_frames = 0;
	u64 generation;
	unsigned long flags;
	bool running = false;
	bool usable = false;

	spin_lock_irqsave(&output->stream_lock, flags);
	playback = output->playback_runtime;
	generation = output->playback_generation;
	if (!playback)
		goto out_silence;

	runtime = playback->substream->runtime;
	spin_lock(&playback->clock_lock);
	total_frames = castkms_audio_total_frames_locked(
		playback, now, runtime->rate);
	running = playback->running;
	spin_unlock(&playback->clock_lock);

	usable = runtime->dma_area && runtime->buffer_size &&
		runtime->rate == CASTKMS_AUDIO_TAP_RATE &&
		runtime->format == SNDRV_PCM_FORMAT_S16_LE &&
		runtime->channels == CASTKMS_AUDIO_TAP_CHANNELS;
	if (tap->playback_generation != generation ||
	    total_frames < tap->playback_frames) {
		tap->playback_generation = generation;
		tap->playback_frames = total_frames > frames ?
			total_frames - frames : 0;
	}
	available = total_frames - tap->playback_frames;
	tap->playback_frames = total_frames;
	if (!usable || !available)
		goto out_silence;

	copy_frames = min(available, frames);
	copy_frames = min_t(u64, copy_frames, runtime->buffer_size);
	first_frame = total_frames - copy_frames;
	if (running) {
		leading_silence = frames - copy_frames;
	} else {
		leading_silence = 0;
		trailing_silence = frames - copy_frames;
	}

out_silence:
	castkms_audio_tap_append_silence(tap, leading_silence);
	if (copy_frames)
		castkms_audio_tap_append_playback(
			tap, runtime, first_frame, copy_frames);
	castkms_audio_tap_append_silence(tap, trailing_silence);
	spin_unlock_irqrestore(&output->stream_lock, flags);
}

static enum hrtimer_restart castkms_audio_tap_timer(struct hrtimer *timer)
{
	struct castkms_audio_tap *tap =
		container_of(timer, struct castkms_audio_tap, timer);
	ktime_t now = ktime_get();
	u64 elapsed_ns;
	u64 expected_frames;
	u64 frames;

	if (castkms_audio_tap_is_terminal(tap))
		return HRTIMER_NORESTART;

	elapsed_ns = ktime_to_ns(ktime_sub(now, tap->base_time));
	expected_frames = mul_u64_u32_div(
		elapsed_ns, CASTKMS_AUDIO_TAP_RATE, NSEC_PER_SEC);
	frames = expected_frames - tap->produced_frames;
	if (frames > CASTKMS_AUDIO_TAP_PERIOD_FRAMES * 4U) {
		unsigned long flags;
		u64 skipped = frames - CASTKMS_AUDIO_TAP_PERIOD_FRAMES * 4U;

		spin_lock_irqsave(&tap->fifo_lock, flags);
		tap->dropped_frames += skipped;
		spin_unlock_irqrestore(&tap->fifo_lock, flags);
		frames -= skipped;
	}
	if (frames)
		castkms_audio_tap_render(tap, now, frames);
	tap->produced_frames = expected_frames;

	hrtimer_forward_now(timer, tap->period_duration);
	if (castkms_audio_tap_is_terminal(tap))
		return HRTIMER_NORESTART;
	return HRTIMER_RESTART;
}

static bool castkms_audio_tap_resource_needs_cleanup(
	struct castkms_capture_authority_resource *resource,
	enum castkms_capture_authority_cleanup_reason reason, u64 generation)
{
	(void)resource;
	(void)reason;
	(void)generation;
	return true;
}

static void castkms_audio_tap_resource_revoke(
	struct castkms_capture_authority_resource *resource, int status)
{
	struct castkms_audio_tap *tap = container_of(
		resource, struct castkms_audio_tap, authority_resource);
	struct castkms_audio_card *output = tap->output;

	mutex_lock(&output->lock);
	if (output->tap == tap)
		output->tap = NULL;
	mutex_unlock(&output->lock);
	castkms_audio_tap_terminate(tap, status);
}

static const struct castkms_capture_authority_resource_ops
castkms_audio_tap_resource_ops = {
	.needs_cleanup = castkms_audio_tap_resource_needs_cleanup,
	.revoke = castkms_audio_tap_resource_revoke,
};

static bool castkms_audio_tap_read_ready(struct castkms_audio_tap *tap)
{
	unsigned long flags;
	bool ready;

	spin_lock_irqsave(&tap->fifo_lock, flags);
	ready = tap->terminal_status ||
		kfifo_len(&tap->fifo) >= CASTKMS_AUDIO_TAP_FRAME_BYTES;
	spin_unlock_irqrestore(&tap->fifo_lock, flags);

	return ready;
}

static ssize_t castkms_audio_tap_read(struct file *file, char __user *buffer,
				      size_t count, loff_t *offset)
{
	struct castkms_audio_tap *tap = file->private_data;
	unsigned int copied;
	unsigned int available;
	unsigned int requested;
	unsigned long flags;
	int status;
	int ret;

	if (count < CASTKMS_AUDIO_TAP_FRAME_BYTES)
		return -EINVAL;
	requested = min_t(size_t, count, 64U * 1024U);
	requested -= requested % CASTKMS_AUDIO_TAP_FRAME_BYTES;
	ret = mutex_lock_interruptible(&tap->read_lock);
	if (ret)
		return ret;
	for (;;) {
		spin_lock_irqsave(&tap->fifo_lock, flags);
		status = tap->terminal_status;
		available = kfifo_len(&tap->fifo);
		available -= available % CASTKMS_AUDIO_TAP_FRAME_BYTES;
		if (available) {
			copied = min(requested, available);
			copied = kfifo_out(&tap->fifo, tap->read_scratch,
					   copied);
			spin_unlock_irqrestore(&tap->fifo_lock, flags);
			break;
		}
		spin_unlock_irqrestore(&tap->fifo_lock, flags);
		if (status) {
			ret = status;
			goto out_unlock;
		}
		if (file->f_flags & O_NONBLOCK) {
			ret = -EAGAIN;
			goto out_unlock;
		}
		ret = wait_event_interruptible(
			tap->wait, castkms_audio_tap_read_ready(tap));
		if (ret)
			goto out_unlock;
	}

	if (copy_to_user(buffer, tap->read_scratch, copied)) {
		ret = -EFAULT;
		goto out_unlock;
	}
	ret = copied;

out_unlock:
	mutex_unlock(&tap->read_lock);
	return ret;
}

static __poll_t castkms_audio_tap_poll(struct file *file, poll_table *wait)
{
	struct castkms_audio_tap *tap = file->private_data;
	unsigned long flags;
	__poll_t events = 0;

	poll_wait(file, &tap->wait, wait);
	spin_lock_irqsave(&tap->fifo_lock, flags);
	if (kfifo_len(&tap->fifo) >= CASTKMS_AUDIO_TAP_FRAME_BYTES)
		events |= EPOLLIN | EPOLLRDNORM;
	if (tap->terminal_status)
		events |= EPOLLHUP | EPOLLERR;
	spin_unlock_irqrestore(&tap->fifo_lock, flags);

	return events;
}

static int castkms_audio_tap_release(struct inode *inode, struct file *file)
{
	struct castkms_audio_tap *tap = file->private_data;
	struct castkms_audio_card *output = tap->output;

	file->private_data = NULL;
	hrtimer_cancel(&tap->timer);
	castkms_capture_authority_unregister_resource(
		tap->authority, &tap->authority_resource);
	mutex_lock(&output->lock);
	if (output->tap == tap)
		output->tap = NULL;
	mutex_unlock(&output->lock);
	castkms_audio_tap_terminate(tap, -ECANCELED);
	castkms_capture_authority_put(tap->authority);
	castkms_audio_card_put(output);
	kfifo_free(&tap->fifo);
	kfree(tap->read_scratch);
	mutex_destroy(&tap->read_lock);
	kfree(tap);

	return 0;
}

static loff_t castkms_audio_tap_llseek(struct file *file, loff_t offset,
				      int whence)
{
	(void)file;
	(void)offset;
	(void)whence;
	return -ESPIPE;
}

static const struct file_operations castkms_audio_tap_fops = {
	.owner = THIS_MODULE,
	.read = castkms_audio_tap_read,
	.poll = castkms_audio_tap_poll,
	.release = castkms_audio_tap_release,
	.llseek = castkms_audio_tap_llseek,
};

static struct file *castkms_audio_tap_file_create(
	struct castkms_audio_card *output,
	struct castkms_capture_authority *authority, u32 fd_flags)
{
	struct castkms_audio_tap *tap;
	struct file *file;
	int ret;

	tap = kzalloc_obj(*tap);
	if (!tap)
		return ERR_PTR(-ENOMEM);
	tap->output = output;
	tap->authority = authority;
	spin_lock_init(&tap->fifo_lock);
	mutex_init(&tap->read_lock);
	init_waitqueue_head(&tap->wait);
	tap->period_duration = ns_to_ktime(
		div_u64((u64)CASTKMS_AUDIO_TAP_PERIOD_FRAMES * NSEC_PER_SEC,
			CASTKMS_AUDIO_TAP_RATE));
	ret = kfifo_alloc(&tap->fifo, CASTKMS_AUDIO_TAP_BUFFER_BYTES,
			  GFP_KERNEL);
	if (ret)
		goto err_tap;
	tap->read_scratch = kmalloc(64U * 1024U, GFP_KERNEL);
	if (!tap->read_scratch) {
		ret = -ENOMEM;
		goto err_fifo;
	}

	castkms_audio_card_get(output);
	castkms_capture_authority_get(authority);
	ret = castkms_capture_authority_register_resource(
		authority, &tap->authority_resource,
		&castkms_audio_tap_resource_ops);
	if (ret)
		goto err_refs;

	mutex_lock(&output->lock);
	if (!output->card)
		ret = -ENODEV;
	else if (output->tap)
		ret = -EBUSY;
	else
		output->tap = tap;
	mutex_unlock(&output->lock);
	if (ret)
		goto err_resource;

	file = anon_inode_getfile("castkms-audio-tap",
				  &castkms_audio_tap_fops, tap,
				  O_RDONLY | (fd_flags & O_NONBLOCK));
	if (IS_ERR(file)) {
		ret = PTR_ERR(file);
		goto err_output;
	}

	hrtimer_setup(&tap->timer, castkms_audio_tap_timer,
		      CLOCK_MONOTONIC, HRTIMER_MODE_REL_SOFT);
	tap->base_time = ktime_get();
	hrtimer_start(&tap->timer, tap->period_duration,
		      HRTIMER_MODE_REL_SOFT);
	return file;

err_output:
	mutex_lock(&output->lock);
	if (output->tap == tap)
		output->tap = NULL;
	mutex_unlock(&output->lock);
err_resource:
	castkms_capture_authority_unregister_resource(
		authority, &tap->authority_resource);
err_refs:
	castkms_capture_authority_put(authority);
	castkms_audio_card_put(output);
	kfree(tap->read_scratch);
err_fifo:
	kfifo_free(&tap->fifo);
err_tap:
	mutex_destroy(&tap->read_lock);
	kfree(tap);
	return ERR_PTR(ret);
}

struct file *
castkms_audio_open_tap(struct castkms_audio *audio,
		       struct drm_connector *connector,
		       struct castkms_capture_authority *authority,
		       u32 fd_flags, struct castkms_audio_tap_params *params)
{
	struct castkms_audio_card *output = NULL;
	struct castkms_audio_slot *slot;
	struct file *file;

	mutex_lock(&audio->lock);
	slot = castkms_audio_find_slot(audio, connector);
	if (slot && slot->card) {
		output = slot->card;
		castkms_audio_card_get(output);
	}
	mutex_unlock(&audio->lock);
	if (!output)
		return ERR_PTR(-ENODEV);

	file = castkms_audio_tap_file_create(output, authority, fd_flags);
	castkms_audio_card_put(output);
	if (IS_ERR(file))
		return file;

	params->rate = CASTKMS_AUDIO_TAP_RATE;
	params->channels = CASTKMS_AUDIO_TAP_CHANNELS;
	params->frame_bytes = CASTKMS_AUDIO_TAP_FRAME_BYTES;
	params->buffer_frames = CASTKMS_AUDIO_TAP_BUFFER_FRAMES;
	return file;
}

/* --- ELD control --- */

static int castkms_eld_ctl_info(struct snd_kcontrol *kcontrol,
				struct snd_ctl_elem_info *uinfo)
{
	uinfo->type = SNDRV_CTL_ELEM_TYPE_BYTES;
	uinfo->count = MAX_ELD_BYTES;
	return 0;
}

static int castkms_eld_ctl_get(struct snd_kcontrol *kcontrol,
			       struct snd_ctl_elem_value *ucontrol)
{
	struct castkms_audio_card *out = kcontrol->private_data;

	mutex_lock(&out->lock);
	memcpy(ucontrol->value.bytes.data, out->eld, MAX_ELD_BYTES);
	mutex_unlock(&out->lock);

	return 0;
}

static const struct snd_kcontrol_new castkms_eld_ctl_template = {
	.access = SNDRV_CTL_ELEM_ACCESS_READ |
		  SNDRV_CTL_ELEM_ACCESS_VOLATILE,
	.iface = SNDRV_CTL_ELEM_IFACE_PCM,
	.name = "ELD",
	.info = castkms_eld_ctl_info,
	.get = castkms_eld_ctl_get,
};

/* --- Attachment-owned card lifecycle --- */

static struct castkms_audio_slot *
castkms_audio_find_slot(struct castkms_audio *audio,
			struct drm_connector *connector)
{
	unsigned int i;

	for (i = 0; i < audio->num_outputs; i++) {
		if (audio->outputs[i].connector == connector)
			return &audio->outputs[i];
	}

	return NULL;
}

static void castkms_audio_display_name(const struct drm_edid *drm_edid,
				       const char *assigned_name,
				       int index, char *name,
				       size_t name_size)
{
	if (assigned_name && assigned_name[0]) {
		strscpy(name, assigned_name, name_size);
		return;
	}

	if (castkms_display_identity_product_name(drm_edid, name, name_size))
		return;

	snprintf(name, name_size, "Casting Display %d", index + 1);
}

static void castkms_audio_card_invalidate(struct castkms_audio_card *out,
					  int status)
{
	mutex_lock(&out->lock);
	memset(out->eld, 0, sizeof(out->eld));
	out->generation++;
	if (out->active_substream)
		snd_pcm_stop_xrun(out->active_substream);
	castkms_audio_tap_terminate_output_locked(out, status);
	mutex_unlock(&out->lock);
}

static void castkms_audio_card_disconnect(struct castkms_audio_card *out)
{
	struct snd_card *card = out->card;

	castkms_audio_card_invalidate(out, -ENOTCONN);
	snd_jack_report(out->jack, 0);

	/*
	 * Make every existing handle fail immediately, but let ALSA retain the
	 * attachment-owned state until the last handle closes. This keeps detach
	 * non-blocking and permits the slot to acquire a fresh card at once.
	 */
	snd_card_free_when_closed(card);
}

static int castkms_audio_card_create(struct castkms_device *castkmsdev,
				     struct castkms_audio_slot *slot,
				     const u8 eld[MAX_ELD_BYTES],
				     const char *display_name)
{
	struct castkms_audio_card *out;
	struct snd_kcontrol_new eld_ctl;
	struct snd_card *card;
	struct snd_pcm *pcm;
	char card_id[16];
	int ret;

	snprintf(card_id, sizeof(card_id), "CastKMS%d", slot->index);
	ret = snd_card_new(&castkmsdev->faux_dev->dev, -1, card_id,
			   THIS_MODULE, 0, &card);
	if (ret)
		return ret;

	out = kzalloc_obj(*out);
	if (!out) {
		ret = -ENOMEM;
		goto err_card;
	}
	kref_init(&out->ref);
	card->private_data = out;
	card->private_free = castkms_audio_snd_card_free;
	out->card = card;
	mutex_init(&out->lock);
	spin_lock_init(&out->stream_lock);
	memcpy(out->eld, eld, sizeof(out->eld));
	strscpy(out->display_name, display_name, sizeof(out->display_name));

	strscpy(card->driver, "castkms", sizeof(card->driver));
	strscpy(card->shortname, display_name, sizeof(card->shortname));
	strscpy(card->mixername, display_name, sizeof(card->mixername));
	snprintf(card->longname, sizeof(card->longname),
		 "CastKMS output %d audio: %s", slot->index, display_name);

	ret = snd_pcm_new(card, "Virtual HDMI", 0, 1, 0, &pcm);
	if (ret)
		goto err_card;

	pcm->private_data = out;
	strscpy(pcm->name, display_name, sizeof(pcm->name));
	snd_pcm_set_ops(pcm, SNDRV_PCM_STREAM_PLAYBACK, &castkms_pcm_ops);
	snd_pcm_set_managed_buffer_all(pcm, SNDRV_DMA_TYPE_VMALLOC,
				       NULL, 0, 0);
	ret = snd_pcm_add_chmap_ctls(pcm, SNDRV_PCM_STREAM_PLAYBACK,
				     snd_pcm_std_chmaps, 2, 0, NULL);
	if (ret)
		goto err_card;
	ret = snd_jack_new(card, "HDMI/DP,pcm=0", SND_JACK_AVOUT,
			   &out->jack, true, false);
	if (ret)
		goto err_card;

	eld_ctl = castkms_eld_ctl_template;
	eld_ctl.device = 0;
	out->eld_ctl = snd_ctl_new1(&eld_ctl, out);
	if (!out->eld_ctl) {
		ret = -ENOMEM;
		goto err_card;
	}
	ret = snd_ctl_add(card, out->eld_ctl);
	if (ret)
		goto err_card;

	ret = snd_card_register(card);
	if (ret)
		goto err_card;

	slot->card = out;
	snd_jack_report(out->jack, SND_JACK_AVOUT);
	return 0;

err_card:
	snd_card_free(card);
	return ret;
}

static void castkms_audio_card_update_eld(
	struct castkms_audio_card *out, const u8 eld[MAX_ELD_BYTES])
{
	bool changed = false;

	mutex_lock(&out->lock);
	if (memcmp(out->eld, eld, sizeof(out->eld))) {
		memcpy(out->eld, eld, sizeof(out->eld));
		out->generation++;
		changed = true;
		if (out->active_substream)
			snd_pcm_stop_xrun(out->active_substream);
		castkms_audio_tap_terminate_output_locked(out, -ESTALE);
	}
	mutex_unlock(&out->lock);

	if (!changed)
		return;

	snd_ctl_notify_one(out->card, SNDRV_CTL_EVENT_MASK_VALUE,
			   out->eld_ctl, 0);
	snd_jack_report(out->jack, SND_JACK_AVOUT);
}

/* --- Connector notification hooks --- */

void castkms_audio_notify_eld(struct castkms_device *castkmsdev,
			      struct drm_connector *connector,
			      const struct drm_edid *drm_edid,
			      const char *assigned_name)
{
	struct castkms_audio *audio = castkmsdev->audio;
	struct castkms_audio_slot *slot;
	u8 eld[MAX_ELD_BYTES];
	char display_name[80];
	bool available;
	int ret;

	if (!audio)
		return;

	mutex_lock(&connector->eld_mutex);
	memcpy(eld, connector->eld, sizeof(eld));
	available = drm_eld_size(eld) > 0;
	mutex_unlock(&connector->eld_mutex);

	castkms_audio_display_name(drm_edid, assigned_name,
				    drm_connector_to_castkms_connector(connector)
					    ->output_index,
				    display_name, sizeof(display_name));

	mutex_lock(&audio->lock);
	slot = castkms_audio_find_slot(audio, connector);
	if (!slot)
		goto out_unlock;

	if (slot->card &&
	    (!available || strcmp(slot->card->display_name, display_name))) {
		struct castkms_audio_card *old_card = slot->card;

		slot->card = NULL;
		castkms_audio_card_disconnect(old_card);
	}

	if (!available)
		goto out_unlock;

	if (slot->card) {
		castkms_audio_card_update_eld(slot->card, eld);
		goto out_unlock;
	}

	ret = castkms_audio_card_create(castkmsdev, slot, eld, display_name);
	if (ret)
		drm_err(&castkmsdev->drm,
			"failed to create audio card for output %d: %d\n",
			slot->index, ret);

out_unlock:
	mutex_unlock(&audio->lock);
}

void castkms_audio_notify_disconnect(struct castkms_device *castkmsdev,
				     struct drm_connector *connector)
{
	struct castkms_audio *audio = castkmsdev->audio;
	struct castkms_audio_slot *slot;
	struct castkms_audio_card *card;

	if (!audio)
		return;

	mutex_lock(&audio->lock);
	slot = castkms_audio_find_slot(audio, connector);
	if (!slot || !slot->card)
		goto out_unlock;

	card = slot->card;
	slot->card = NULL;
	castkms_audio_card_disconnect(card);

out_unlock:
	mutex_unlock(&audio->lock);
}

/* --- Device-level init/cleanup --- */

int castkms_audio_init(struct castkms_device *castkmsdev)
{
	struct castkms_audio *audio;
	struct drm_connector_list_iter iter;
	struct drm_connector *connector;
	unsigned int num_outputs = 0;
	unsigned int idx = 0;

	drm_connector_list_iter_begin(&castkmsdev->drm, &iter);
	drm_for_each_connector_iter(connector, &iter) {
		if (connector->connector_type != DRM_MODE_CONNECTOR_WRITEBACK)
			num_outputs++;
	}
	drm_connector_list_iter_end(&iter);

	if (!num_outputs)
		return 0;

	audio = kzalloc(sizeof(*audio), GFP_KERNEL);
	if (!audio)
		return -ENOMEM;

	audio->outputs = kcalloc(num_outputs, sizeof(*audio->outputs),
				 GFP_KERNEL);
	if (!audio->outputs) {
		kfree(audio);
		return -ENOMEM;
	}
	mutex_init(&audio->lock);
	audio->num_outputs = num_outputs;

	drm_connector_list_iter_begin(&castkmsdev->drm, &iter);
	drm_for_each_connector_iter(connector, &iter) {
		struct castkms_connector *castkms_conn;

		if (connector->connector_type == DRM_MODE_CONNECTOR_WRITEBACK)
			continue;
		if (idx >= num_outputs)
			break;

		castkms_conn = drm_connector_to_castkms_connector(connector);
		audio->outputs[idx].connector = connector;
		audio->outputs[idx].index = castkms_conn->output_index;
		idx++;
	}
	drm_connector_list_iter_end(&iter);

	castkmsdev->audio = audio;
	return 0;
}

void castkms_audio_cleanup(struct castkms_device *castkmsdev)
{
	struct castkms_audio *audio = castkmsdev->audio;
	unsigned int i;

	if (!audio)
		return;

	mutex_lock(&audio->lock);
	for (i = 0; i < audio->num_outputs; i++) {
		struct castkms_audio_card *card = audio->outputs[i].card;

		if (!card)
			continue;
		audio->outputs[i].card = NULL;
		castkms_audio_card_disconnect(card);
	}
	mutex_unlock(&audio->lock);

	castkmsdev->audio = NULL;
	kfree(audio->outputs);
	kfree(audio);
}
