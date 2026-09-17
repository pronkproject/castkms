// SPDX-License-Identifier: GPL-2.0-only
/* Run only on a disposable eight-output CastKMS device without a sound server. */
#define _GNU_SOURCE
#include "audio-fixture.h"
#include <fcntl.h>
#include <limits.h>
#include <poll.h>
#include <signal.h>
#include <sound/asound.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/wait.h>
#include <unistd.h>

#define OUTPUTS 8

struct output {
	uint32_t crtc, connector;
	drmModeModeInfo mode;
	struct buffer buffer;
	struct drm_castkms_create_monitor_control monitor;
	struct drm_castkms_monitor_files monitor_files;
	struct drm_castkms_audio_files audio;
	pid_t writer;
};

static struct output outputs[OUTPUTS];
static pid_t parent;

static void stop_writer(struct output *output)
{
	if (output->writer > 0) {
		pid_t ret;

		do {
			ret = waitpid(output->writer, NULL, WNOHANG);
		} while (ret < 0 && errno == EINTR);
		if (ret == 0) {
			kill(output->writer, SIGTERM);
			while (waitpid(output->writer, NULL, 0) < 0 && errno == EINTR)
				;
		}
		output->writer = 0;
	}
}

static void cleanup(void)
{
	if (getpid() != parent)
		return;
	for (unsigned int i = 0; i < OUTPUTS; i++)
		stop_writer(&outputs[i]);
}

static void fixed(struct snd_pcm_hw_params *hw, unsigned int param, unsigned int value)
{
	struct snd_interval *interval = &hw->intervals[param - SNDRV_PCM_HW_PARAM_FIRST_INTERVAL];

	interval->min = interval->max = value;
	interval->integer = 1;
}

static void start_writer(unsigned int output)
{
	struct snd_pcm_hw_params hw = { .rmask = ~0U };
	struct snd_pcm_sw_params sw = {
		.period_step = 1, .avail_min = 480, .start_threshold = 4800,
		.stop_threshold = 4800, .boundary = 1UL << 30,
	};
	unsigned short samples[960];
	char path[64];
	int fd, card;

	outputs[output].writer = fork();
	CHECK(outputs[output].writer >= 0);
	if (outputs[output].writer)
		return;
	/* Writers must not keep any DRM creator or revocation files alive. */
	CHECK(close_range(3, ~0U, 0) == 0);
	card = audio_card(output);
	CHECK(card >= 0);
	snprintf(path, sizeof(path), "/dev/snd/pcmC%dD0p", card);
	fd = open(path, O_WRONLY | O_CLOEXEC);
	CHECK(fd >= 0);
	for (unsigned int i = 0; i < sizeof(hw.intervals) / sizeof(hw.intervals[0]); i++)
		hw.intervals[i].max = UINT_MAX;
	hw.masks[SNDRV_PCM_HW_PARAM_ACCESS].bits[0] = 1U << SNDRV_PCM_ACCESS_RW_INTERLEAVED;
	hw.masks[SNDRV_PCM_HW_PARAM_FORMAT].bits[0] = 1U << SNDRV_PCM_FORMAT_S16_LE;
	hw.masks[SNDRV_PCM_HW_PARAM_SUBFORMAT].bits[0] = 1U << SNDRV_PCM_SUBFORMAT_STD;
	fixed(&hw, SNDRV_PCM_HW_PARAM_RATE, 48000);
	fixed(&hw, SNDRV_PCM_HW_PARAM_CHANNELS, 2);
	fixed(&hw, SNDRV_PCM_HW_PARAM_PERIOD_SIZE, 480);
	fixed(&hw, SNDRV_PCM_HW_PARAM_BUFFER_SIZE, 4800);
	CHECK(ioctl(fd, SNDRV_PCM_IOCTL_HW_PARAMS, &hw) == 0);
	CHECK(ioctl(fd, SNDRV_PCM_IOCTL_SW_PARAMS, &sw) == 0);
	CHECK(ioctl(fd, SNDRV_PCM_IOCTL_PREPARE) == 0);
	for (unsigned int i = 0; i < 960; i += 2) {
		samples[i] = 0x1000 + output;
		samples[i + 1] = 0x2000 + output;
	}
	for (;;) {
		struct snd_xferi transfer = { .buf = samples, .frames = 480 };
		int ret = ioctl(fd, SNDRV_PCM_IOCTL_WRITEI_FRAMES, &transfer);
		int error = ret < 0 ? errno : transfer.result < 0 ? -transfer.result : 0;

		if (!error)
			continue;
		if (error == ENODEV || error == EBADFD)
			_exit(0);
		CHECK(error == EPIPE || error == ESTRPIPE || error == EINTR);
		while (ioctl(fd, SNDRV_PCM_IOCTL_PREPARE) < 0) {
			if (errno == ENODEV)
				_exit(0);
			CHECK(errno == EPIPE);
			usleep(1000);
		}
	}
}

static void signal_on(unsigned int index)
{
	unsigned short samples[960];
	int fd = outputs[index].audio.audio_fd;
	int status;
	pid_t ret = waitpid(outputs[index].writer, &status, WNOHANG);

	if (ret == outputs[index].writer)
		outputs[index].writer = 0;
	CHECK(ret == 0);

	for (unsigned int attempt = 0; attempt < 200; attempt++) {
		struct pollfd event = { .fd = fd, .events = POLLIN };
		ssize_t count;
		int found = 0;

		CHECK(poll(&event, 1, 20) >= 0);
		CHECK(!(event.revents & (POLLHUP | POLLERR)));
		count = read(fd, samples, sizeof(samples));
		if (count < 0 && errno == EAGAIN)
			continue;
		CHECK(count > 0 && count % 4 == 0);
		for (ssize_t i = 0; i < count / 2; i += 2) {
			if (!samples[i] && !samples[i + 1])
				continue;
			CHECK(samples[i] == 0x1000 + index && samples[i + 1] == 0x2000 + index);
			found = 1;
		}
		if (found)
			return;
	}
	CHECK(!"missing per-output audio signal");
}

static void enable(int fd, struct output *output)
{
	CHECK(drmModeSetCrtc(fd, output->crtc, output->buffer.fb, 0, 0,
			     &output->connector, 1, &output->mode) == 0);
}

int main(int argc, char **argv)
{
	struct drm_castkms_monitor_attach attach = {0};
	struct drm_castkms_monitor_detach detach = {0};
	unsigned char edid[256];
	drmModeRes *resources;
	drmVersion *version;
	int fd, successor;

	if (argc != 2) {
		fprintf(stderr, "SKIP: supply a disposable eight-output CastKMS node without a sound server\n");
		return 4;
	}
	CHECK(atexit(cleanup) == 0);
	parent = getpid();
	fd = open(argv[1], O_RDWR | O_CLOEXEC);
	CHECK(fd >= 0);
	version = drmGetVersion(fd);
	CHECK(version && !strcmp(version->name, "castkms"));
	drmFreeVersion(version);
	CHECK(drmSetMaster(fd) == 0);
	resources = drmModeGetResources(fd);
	CHECK(resources && resources->count_crtcs == OUTPUTS && resources->count_connectors == OUTPUTS);
	audio_edid(edid);
	attach.edid_ptr = (uintptr_t)edid;
	attach.edid_size = sizeof(edid);
	for (unsigned int i = 0; i < OUTPUTS; i++) {
		struct output *output = &outputs[i];
		drmModeConnector *connector;

		output->crtc = resources->crtcs[i];
		output->connector = resources->connectors[i];
		output->monitor.connector_id = output->connector;
		output->monitor.files = (uintptr_t)&output->monitor_files;
		CHECK(ioctl(fd, DRM_IOCTL_CASTKMS_CREATE_MONITOR_CONTROL, &output->monitor) == 0);
		CHECK(ioctl(output->monitor_files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_ATTACH, &attach) == 0);
		connector = drmModeGetConnector(fd, output->connector);
		CHECK(connector && connector->count_modes > 0);
		output->mode = connector->modes[0];
		for (int m = 1; m < connector->count_modes; m++)
			if (connector->modes[m].hdisplay * connector->modes[m].vdisplay <
			    output->mode.hdisplay * output->mode.vdisplay)
				output->mode = connector->modes[m];
		drmModeFreeConnector(connector);
		output->buffer = create_buffer(fd, output->mode.hdisplay, output->mode.vdisplay, 0);
		enable(fd, output);
		output->audio = audio_capture(fd, output->crtc, output->connector);
		start_writer(i);
	}
	drmModeFreeResources(resources);
	{
		struct drm_castkms_audio_files files = { -1, -1 };
		struct drm_castkms_create_audio_capture request = {
			.crtc_id = outputs[0].crtc, .connector_id = outputs[1].connector,
			.files = (uintptr_t)&files,
		};

		CHECK(ioctl(fd, DRM_IOCTL_CASTKMS_CREATE_AUDIO_CAPTURE, &request) < 0 && errno == EACCES);
		CHECK(files.audio_fd == -1 && files.revoke_fd == -1);
	}
	for (unsigned int round = 0; round < 8; round++) {
		for (unsigned int i = 0; i < OUTPUTS; i++) {
			struct output *output = &outputs[i];
			struct pollfd event = { .fd = output->audio.audio_fd, .events = POLLIN };

			signal_on(i);
			CHECK(drmModeSetCrtc(fd, output->crtc, 0, 0, 0, NULL, 0, NULL) == 0);
			CHECK(poll(&event, 1, 20) == 0);
			for (unsigned int other = 0; other < OUTPUTS; other++)
				if (other != i)
					signal_on(other);
			enable(fd, output);
			signal_on(i);
		}
	}
	for (unsigned int i = 0; i < OUTPUTS; i++) {
		struct output *output = &outputs[i];
		struct drm_castkms_audio_files old = output->audio;

		CHECK(ioctl(output->monitor_files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_DETACH, &detach) == 0);
		audio_terminal(old.audio_fd);
		stop_writer(output);
		CHECK(ioctl(output->monitor_files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_ATTACH, &attach) == 0);
		output->audio = audio_capture(fd, output->crtc, output->connector);
		start_writer(i);
		for (unsigned int other = 0; other < OUTPUTS; other++)
			signal_on(other);
		audio_terminal(old.audio_fd);
		CHECK(close(old.audio_fd) == 0 && close(old.revoke_fd) == 0);
	}
	/* Open before dropping master so asynchronous recovery cannot reject open. */
	successor = open(argv[1], O_RDWR | O_CLOEXEC);
	CHECK(successor >= 0);
	CHECK(drmDropMaster(fd) == 0);
	for (unsigned int i = 0; i < OUTPUTS; i++)
		audio_terminal(outputs[i].audio.audio_fd);
	acquire_master(successor);
	for (unsigned int i = 0; i < OUTPUTS; i++) {
		struct output *output = &outputs[i];
		struct drm_castkms_audio_files old = output->audio;

		output->audio = audio_capture(successor, output->crtc, output->connector);
		enable(successor, output);
		signal_on(i);
		audio_terminal(old.audio_fd);
		CHECK(close(old.audio_fd) == 0 && close(old.revoke_fd) == 0);
	}
	CHECK(close(successor) == 0);
	for (unsigned int i = 0; i < OUTPUTS; i++) {
		struct output *output = &outputs[i];

		audio_terminal(output->audio.audio_fd);
		stop_writer(output);
		CHECK(close(output->audio.audio_fd) == 0 && close(output->audio.revoke_fd) == 0);
		CHECK(close(output->monitor_files.control_fd) == 0 && close(output->monitor_files.revoke_fd) == 0);
		destroy_buffer(fd, &output->buffer);
	}
	CHECK(close(fd) == 0);
	puts("PASS: eight audio outputs, 64 modesets, hotplug isolation and master handoff");
	return 0;
}
