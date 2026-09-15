// SPDX-License-Identifier: GPL-2.0-only
/* Run only on a disposable CastKMS device with one output and audio enabled. */
#include "audio-fixture.h"
#include <fcntl.h>
#include <limits.h>
#include <poll.h>
#include <sound/asound.h>
#include <string.h>
#include <sys/ioctl.h>
#include <unistd.h>
#include "../../../../include/uapi/drm/castkms_drm.h"

_Static_assert(sizeof(struct drm_castkms_create_audio_capture) == 32, "create ABI");
_Static_assert(sizeof(struct drm_castkms_audio_files) == 8, "files ABI");
_Static_assert(sizeof(struct drm_castkms_audio_query) == 40, "query ABI");

static void interval(struct snd_pcm_hw_params *params, unsigned int which, unsigned int value)
{
	struct snd_interval *entry = &params->intervals[which - SNDRV_PCM_HW_PARAM_FIRST_INTERVAL];

	entry->min = entry->max = value;
	entry->integer = 1;
}

static int playback(void)
{
	struct snd_pcm_hw_params hw = { .rmask = ~0U };
	struct snd_pcm_sw_params sw = {
		.period_step = 1, .avail_min = 480, .start_threshold = 480,
		.stop_threshold = 4800, .boundary = 1UL << 30,
	};
	unsigned short samples[9600];
	struct snd_xferi transfer = { .buf = samples, .frames = 4800 };
	char path[64];
	int fd = -1;

	for (unsigned int i = 0; i < 32; i++) {
		struct snd_ctl_card_info info = { 0 };
		int control, ret;

		snprintf(path, sizeof(path), "/dev/snd/controlC%u", i);
		control = open(path, O_RDONLY | O_CLOEXEC);
		if (control < 0)
			continue;
		ret = ioctl(control, SNDRV_CTL_IOCTL_CARD_INFO, &info);

		close(control);
		if (ret || strcmp((char *)info.driver, "castkms"))
			continue;
		snprintf(path, sizeof(path), "/dev/snd/pcmC%uD0p", i);
		fd = open(path, O_WRONLY | O_CLOEXEC);
		if (fd >= 0)
			break;
	}
	CHECK(fd >= 0);
	for (unsigned int i = 0; i < sizeof(hw.intervals) / sizeof(hw.intervals[0]); i++)
		hw.intervals[i].max = UINT_MAX;
	hw.masks[SNDRV_PCM_HW_PARAM_ACCESS].bits[0] = 1U << SNDRV_PCM_ACCESS_RW_INTERLEAVED;
	hw.masks[SNDRV_PCM_HW_PARAM_FORMAT].bits[0] = 1U << SNDRV_PCM_FORMAT_S16_LE;
	hw.masks[SNDRV_PCM_HW_PARAM_SUBFORMAT].bits[0] = 1U << SNDRV_PCM_SUBFORMAT_STD;
	interval(&hw, SNDRV_PCM_HW_PARAM_CHANNELS, 2);
	interval(&hw, SNDRV_PCM_HW_PARAM_RATE, 48000);
	interval(&hw, SNDRV_PCM_HW_PARAM_PERIOD_SIZE, 480);
	interval(&hw, SNDRV_PCM_HW_PARAM_BUFFER_SIZE, 4800);
	CHECK(ioctl(fd, SNDRV_PCM_IOCTL_HW_PARAMS, &hw) == 0);
	CHECK(ioctl(fd, SNDRV_PCM_IOCTL_SW_PARAMS, &sw) == 0);
	CHECK(ioctl(fd, SNDRV_PCM_IOCTL_PREPARE) == 0);
	for (unsigned int i = 0; i < 9600; i += 2) {
		samples[i] = 0x1234;
		samples[i + 1] = 0x5678;
	}
	CHECK(ioctl(fd, SNDRV_PCM_IOCTL_WRITEI_FRAMES, &transfer) == 0);
	CHECK(transfer.result == 4800);
	return fd;
}

static void malformed(int fd, uint32_t crtc, uint32_t connector)
{
	struct drm_castkms_create_audio_capture request = {
		.crtc_id = crtc, .connector_id = connector, .files = 1,
	};

	CHECK(ioctl(fd, DRM_IOCTL_CASTKMS_CREATE_AUDIO_CAPTURE, &request) < 0 && errno == EFAULT);
	request.reserved[0] = 1;
	CHECK(ioctl(fd, DRM_IOCTL_CASTKMS_CREATE_AUDIO_CAPTURE, &request) < 0 && errno == EINVAL);
	request.reserved[0] = 0;
	request.flags = 2;
	CHECK(ioctl(fd, DRM_IOCTL_CASTKMS_CREATE_AUDIO_CAPTURE, &request) < 0 && errno == EINVAL);
}

static void pause_resume(int fd)
{
	struct snd_pcm_status before = {0}, after = {0};

	CHECK(ioctl(fd, SNDRV_PCM_IOCTL_PAUSE, 1) == 0);
	CHECK(ioctl(fd, SNDRV_PCM_IOCTL_STATUS, &before) == 0);
	CHECK(before.state == SNDRV_PCM_STATE_PAUSED);
	usleep(20000);
	CHECK(ioctl(fd, SNDRV_PCM_IOCTL_STATUS, &after) == 0);
	CHECK(before.hw_ptr == after.hw_ptr);
	CHECK(ioctl(fd, SNDRV_PCM_IOCTL_PAUSE, 0) == 0);
	usleep(5000);
	CHECK(ioctl(fd, SNDRV_PCM_IOCTL_STATUS, &after) == 0);
	CHECK(after.state == SNDRV_PCM_STATE_RUNNING && after.hw_ptr > before.hw_ptr);
}

static void reconfigure_prepared(int fd)
{
	CHECK(ioctl(fd, SNDRV_PCM_IOCTL_DROP) == 0);
	for (unsigned int attempt = 0; attempt < 32; attempt++) {
		struct snd_pcm_hw_params hw = { .rmask = ~0U };

		for (unsigned int i = 0; i < sizeof(hw.intervals) / sizeof(hw.intervals[0]); i++)
			hw.intervals[i].max = UINT_MAX;
		hw.masks[SNDRV_PCM_HW_PARAM_ACCESS].bits[0] = 1U << SNDRV_PCM_ACCESS_RW_INTERLEAVED;
		hw.masks[SNDRV_PCM_HW_PARAM_FORMAT].bits[0] = 1U << SNDRV_PCM_FORMAT_S16_LE;
		hw.masks[SNDRV_PCM_HW_PARAM_SUBFORMAT].bits[0] = 1U << SNDRV_PCM_SUBFORMAT_STD;
		interval(&hw, SNDRV_PCM_HW_PARAM_CHANNELS, 2);
		interval(&hw, SNDRV_PCM_HW_PARAM_RATE, 48000);
		interval(&hw, SNDRV_PCM_HW_PARAM_PERIOD_SIZE, 480);
		interval(&hw, SNDRV_PCM_HW_PARAM_BUFFER_SIZE, attempt % 2 ? 960 : 4800);
		CHECK(ioctl(fd, SNDRV_PCM_IOCTL_HW_PARAMS, &hw) == 0);
		CHECK(ioctl(fd, SNDRV_PCM_IOCTL_PREPARE) == 0);
		usleep(1000);
	}
	CHECK(ioctl(fd, SNDRV_PCM_IOCTL_HW_FREE) == 0);
}

int main(int argc, char **argv)
{
	struct drm_castkms_create_monitor_control monitor = { .control_fd = -1 };
	struct drm_castkms_monitor_attach attach = {0};
	struct drm_castkms_monitor_detach detach = {0};
	struct drm_castkms_audio_query query = {0};
	struct drm_castkms_audio_files audio, next;
	drmModeRes *resources;
	drmVersion *version;
	drmModeConnector *connector;
	drmModeModeInfo mode;
	unsigned char edid[256], samples[1920];
	struct buffer buffer;
	uint32_t crtc_id, connector_id;
	int fd, pcm, found = 0;

	if (argc != 2) {
		fprintf(stderr, "SKIP: supply a disposable CastKMS DRM node\n");
		return 4;
	}
	fd = open(argv[1], O_RDWR | O_CLOEXEC);
	CHECK(fd >= 0);
	version = drmGetVersion(fd);
	CHECK(version && !strcmp(version->name, "castkms"));
	drmFreeVersion(version);
	CHECK(drmSetMaster(fd) == 0);
	resources = drmModeGetResources(fd);
	CHECK(resources && resources->count_crtcs == 1 && resources->count_connectors == 1);
	crtc_id = resources->crtcs[0];
	connector_id = resources->connectors[0];
	drmModeFreeResources(resources);
	monitor.connector_id = connector_id;
	CHECK(ioctl(fd, DRM_IOCTL_CASTKMS_CREATE_MONITOR_CONTROL, &monitor) == 0);
	audio_edid(edid);
	attach.edid_ptr = (uintptr_t)edid;
	attach.edid_size = sizeof(edid);
	CHECK(ioctl(monitor.control_fd, DRM_IOCTL_CASTKMS_MONITOR_ATTACH, &attach) == 0);
	connector = drmModeGetConnector(fd, connector_id);
	CHECK(connector && connector->count_modes > 0);
	mode = connector->modes[0];
	buffer = create_buffer(fd, connector->modes[0].hdisplay, connector->modes[0].vdisplay, 0);
	CHECK(drmModeSetCrtc(fd, crtc_id, buffer.fb, 0, 0, &connector_id, 1, &connector->modes[0]) == 0);
	drmModeFreeConnector(connector);
	malformed(fd, crtc_id, connector_id);
	audio = audio_capture(fd, crtc_id, connector_id);
	CHECK(ioctl(audio.audio_fd, DRM_IOCTL_CASTKMS_AUDIO_QUERY, &query) == 0);
	CHECK(query.version == 1 && query.rate == 48000 && query.channels == 2 && query.frame_bytes == 4);
	CHECK(query.format == DRM_CASTKMS_AUDIO_S16_LE && query.buffer_frames == 65536 && !query.reserved);
	CHECK(ioctl(audio.revoke_fd, DRM_IOCTL_CASTKMS_AUDIO_QUERY, &query) < 0 && errno == ENOTTY);
	CHECK(read(audio.audio_fd, samples, 3) < 0 && errno == EINVAL);
	usleep(30000);
	CHECK(read(audio.audio_fd, samples, sizeof(samples)) == sizeof(samples));
	for (unsigned int i = 0; i < sizeof(samples); i++)
		CHECK(samples[i] == 0);
	pcm = playback();
	for (unsigned int attempt = 0; attempt < 20 && !found; attempt++) {
		struct pollfd event = { .fd = audio.audio_fd, .events = POLLIN };
		ssize_t count;

		CHECK(poll(&event, 1, 1000) == 1 && (event.revents & POLLIN));
		count = read(audio.audio_fd, samples, sizeof(samples));
		CHECK(count > 0 && count % 4 == 0);
		for (ssize_t i = 0; i < count; i += 4)
			if (!memcmp(samples + i, "\x34\x12\x78\x56", 4))
				found = 1;
	}
	CHECK(found);
	pause_resume(pcm);
	CHECK(drmModeSetCrtc(fd, crtc_id, 0, 0, 0, NULL, 0, NULL) == 0);
	{
		struct pollfd event = { .fd = audio.audio_fd, .events = POLLIN };
		struct snd_pcm_status status = {0};

		CHECK(poll(&event, 1, 30) == 0);
		CHECK(read(audio.audio_fd, samples, sizeof(samples)) < 0 && errno == EAGAIN);
		CHECK(ioctl(audio.audio_fd, DRM_IOCTL_CASTKMS_AUDIO_QUERY, &query) == 0);
		CHECK(ioctl(pcm, SNDRV_PCM_IOCTL_STATUS, &status) == 0);
		CHECK(status.state == SNDRV_PCM_STATE_XRUN);
	}
	CHECK(drmModeSetCrtc(fd, crtc_id, buffer.fb, 0, 0, &connector_id, 1, &mode) == 0);
	reconfigure_prepared(pcm);
	CHECK(ioctl(monitor.control_fd, DRM_IOCTL_CASTKMS_MONITOR_DETACH, &detach) == 0);
	audio_terminal(audio.audio_fd);
	CHECK(ioctl(pcm, SNDRV_PCM_IOCTL_PREPARE) < 0);
	CHECK(close(pcm) == 0);
	CHECK(ioctl(monitor.control_fd, DRM_IOCTL_CASTKMS_MONITOR_ATTACH, &attach) == 0);
	next = audio_capture(fd, crtc_id, connector_id);
	audio_terminal(audio.audio_fd);
	CHECK(close(next.revoke_fd) == 0);
	audio_terminal(next.audio_fd);
	CHECK(close(next.audio_fd) == 0);
	CHECK(close(audio.audio_fd) == 0 && close(audio.revoke_fd) == 0);
	next = audio_capture(fd, crtc_id, connector_id);
	CHECK(close(fd) == 0);
	audio_terminal(next.audio_fd);
	CHECK(close(next.audio_fd) == 0 && close(next.revoke_fd) == 0);
	CHECK(close(monitor.control_fd) == 0 && close(monitor.revoke_fd) == 0);
	puts("PASS: audio playback, silence, attachment replacement, revocation and creator close");
	return 0;
}
