// SPDX-License-Identifier: GPL-2.0+

#include <linux/device/faux.h>
#include <linux/hrtimer.h>
#include <linux/math64.h>
#include <linux/mutex.h>
#include <linux/slab.h>
#include <linux/string.h>

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
#include "castkms_config.h"
#include "castkms_connector.h"
#include "castkms_device.h"
#include "castkms_display_identity.h"

struct castkms_audio_card {
	struct snd_card *card;
	struct snd_jack *jack;
	struct snd_kcontrol *eld_ctl;
	char display_name[80];

	struct mutex lock;
	u8 eld[MAX_ELD_BYTES];
	unsigned int generation;
	struct snd_pcm_substream *active_substream;
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
	ktime_t period_duration;
	ktime_t base_time;
	u64 base_frames;
	unsigned int eld_generation;
	uint8_t eld[MAX_ELD_BYTES];
	bool running;
};

static const struct snd_pcm_hardware castkms_pcm_hw = {
	.info = SNDRV_PCM_INFO_INTERLEAVED |
		SNDRV_PCM_INFO_MMAP |
		SNDRV_PCM_INFO_MMAP_VALID |
		SNDRV_PCM_INFO_PAUSE |
		SNDRV_PCM_INFO_HAS_LINK_ESTIMATED_ATIME,
	.formats = SNDRV_PCM_FMTBIT_S16_LE,
	.rates = SNDRV_PCM_RATE_32000 |
		 SNDRV_PCM_RATE_44100 |
		 SNDRV_PCM_RATE_48000,
	.rate_min = 32000,
	.rate_max = 48000,
	.channels_min = 2,
	.channels_max = 2,
	.period_bytes_min = 64 * 2 * 2,
	.period_bytes_max = 16384 * 2 * 2,
	.periods_min = 2,
	.periods_max = 32,
	.buffer_bytes_max = 65536 * 2 * 2,
};

/* --- frame position --- */

static u64 castkms_audio_frames_since_base(struct castkms_audio_runtime *rt,
					    ktime_t now, unsigned int rate)
{
	u64 elapsed_ns;

	elapsed_ns = ktime_to_ns(ktime_sub(now, rt->base_time));
	return mul_u64_u32_div(elapsed_ns, rate, NSEC_PER_SEC);
}

static u64 castkms_audio_total_frames(struct castkms_audio_runtime *rt,
				       ktime_t now, unsigned int rate)
{
	if (!READ_ONCE(rt->running))
		return rt->base_frames;

	return rt->base_frames +
	       castkms_audio_frames_since_base(rt, now, rate);
}

/* --- hrtimer --- */

static enum hrtimer_restart castkms_audio_timer(struct hrtimer *timer)
{
	struct castkms_audio_runtime *rt =
		container_of(timer, struct castkms_audio_runtime, timer);

	if (!READ_ONCE(rt->running))
		return HRTIMER_NORESTART;

	hrtimer_forward_now(timer, rt->period_duration);
	snd_pcm_period_elapsed(rt->substream);

	if (!READ_ONCE(rt->running))
		return HRTIMER_NORESTART;
	return HRTIMER_RESTART;
}

static void castkms_audio_timer_arm(struct castkms_audio_runtime *rt)
{
	WRITE_ONCE(rt->running, true);
	rt->base_time = ktime_get();
	hrtimer_start(&rt->timer, rt->period_duration, HRTIMER_MODE_REL_SOFT);
}

static void castkms_audio_timer_disarm(struct castkms_audio_runtime *rt,
				       unsigned int rate)
{
	ktime_t now = ktime_get();

	rt->base_frames = castkms_audio_total_frames(rt, now, rate);
	WRITE_ONCE(rt->running, false);
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

	if (READ_ONCE(rt->running))
		castkms_audio_timer_disarm(rt, substream->runtime->rate);
	hrtimer_cancel(&rt->timer);

	mutex_lock(&out->lock);
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
	rt->period_duration = ns_to_ktime(period_ns);
	rt->base_frames = 0;
	rt->base_time = ktime_set(0, 0);

	return 0;
}

static int castkms_pcm_trigger(struct snd_pcm_substream *substream, int cmd)
{
	struct castkms_audio_runtime *rt = substream->runtime->private_data;
	unsigned int rate = substream->runtime->rate;

	switch (cmd) {
	case SNDRV_PCM_TRIGGER_START:
		rt->base_frames = 0;
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
	.prepare = castkms_pcm_prepare,
	.trigger = castkms_pcm_trigger,
	.pointer = castkms_pcm_pointer,
	.get_time_info = castkms_pcm_get_time_info,
};

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

static void castkms_audio_card_invalidate(struct castkms_audio_card *out)
{
	mutex_lock(&out->lock);
	memset(out->eld, 0, sizeof(out->eld));
	out->generation++;
	if (out->active_substream)
		snd_pcm_stop_xrun(out->active_substream);
	mutex_unlock(&out->lock);
}

static void castkms_audio_card_disconnect(struct castkms_audio_card *out)
{
	struct snd_card *card = out->card;

	castkms_audio_card_invalidate(out);
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
			   THIS_MODULE, sizeof(*out), &card);
	if (ret)
		return ret;

	out = card->private_data;
	out->card = card;
	mutex_init(&out->lock);
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
