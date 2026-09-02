// SPDX-License-Identifier: GPL-2.0-only
/*
 * Validate the grant-scoped CastKMS audio tap against live ALSA playback.
 *
 * The inherited grant owns the temporary monitor attachment. Playback is
 * started before the tap is opened to cover the desktop-first startup order.
 */

#include <alsa/asoundlib.h>
#include <drm/castkms_drm.h>

#include <assert.h>
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <time.h>
#include <unistd.h>

#include "castkms-test-drm.h"
#include "virtualscreen-edid.h"

#define TEST_RATE		48000U
#define TEST_CHANNELS		2U
#define TEST_FRAME_BYTES	4U
#define TEST_PERIOD_FRAMES	480U
#define TEST_PERIOD_BYTES	(TEST_PERIOD_FRAMES * TEST_FRAME_BYTES)
#define TEST_BUFFER_PERIODS	8U
#define TEST_MIN_MATCHING_FRAMES	4800U

static_assert(sizeof(struct drm_castkms_open_audio_tap) == 48,
	      "open-audio-tap ABI size changed");
static_assert(offsetof(struct drm_castkms_open_audio_tap, buffer_frames) == 32,
	      "open-audio-tap buffer-frames offset changed");

static int64_t monotonic_ns(void)
{
	struct timespec now;

	if (clock_gettime(CLOCK_MONOTONIC, &now) < 0)
		return -1;
	return (int64_t)now.tv_sec * 1000000000LL + now.tv_nsec;
}

static int parse_fd(const char *text)
{
	char *end = NULL;
	long value;

	errno = 0;
	value = strtol(text, &end, 10);
	if (errno || !*text || !end || *end || value < 0 || value > INT32_MAX)
		return -1;
	return (int)value;
}

static int open_grant(struct drm_castkms_get_grant *grant)
{
	const uint32_t required_rights =
		DRM_CASTKMS_GRANT_MANAGE_ATTACHMENT |
		DRM_CASTKMS_GRANT_UPDATE_EDID |
		DRM_CASTKMS_GRANT_CAPTURE_AUDIO;
	const char *value = getenv("CASTKMS_GRANT_FD");
	int inherited_fd;
	int fd;

	if (!value || (inherited_fd = parse_fd(value)) < 0) {
		fprintf(stderr, "CASTKMS_GRANT_FD is not a valid descriptor\n");
		return -1;
	}
	fd = fcntl(inherited_fd, F_DUPFD_CLOEXEC, 3);
	if (fd < 0) {
		perror("duplicate grant fd");
		return -1;
	}
	if (castkms_test_check_driver_name(fd))
		goto fail;
	if (ioctl(fd, DRM_IOCTL_CASTKMS_GET_GRANT, grant) < 0) {
		perror("DRM_IOCTL_CASTKMS_GET_GRANT");
		goto fail;
	}
	if (!grant->grant_id || !grant->connector_id ||
	    (grant->rights & required_rights) != required_rights ||
	    grant->rights & ~DRM_CASTKMS_GRANT_RIGHTS_MASK ||
	    grant->state == DRM_CASTKMS_GRANT_STATE_REVOKED ||
	    grant->state > DRM_CASTKMS_GRANT_STATE_REVOKED ||
	    grant->reserved) {
		fprintf(stderr, "inherited fd is not a usable audio grant\n");
		goto fail;
	}
	return fd;

fail:
	close(fd);
	return -1;
}

static int attach_monitor(int fd, uint32_t connector_id)
{
	uint8_t edid[CASTKMS_REFERENCE_EDID_MAX_SIZE];
	static const char display_name[] = "CastKMS audio tap test";
	struct drm_castkms_capture_attach_monitor attach = {
		.connector_id = connector_id,
		.display_name_size = sizeof(display_name) - 1,
		.display_name_ptr = (uint64_t)(uintptr_t)display_name,
	};
	int edid_size;

	edid_size = castkms_fill_edid(edid, sizeof(edid), NULL,
				      CASTKMS_EDID_FLAG_AUDIO);
	if (edid_size < 0) {
		fprintf(stderr, "build audio EDID: %s\n", strerror(-edid_size));
		return -1;
	}
	attach.edid_size = (uint32_t)edid_size;
	attach.edid_ptr = (uint64_t)(uintptr_t)edid;
	if (ioctl(fd, DRM_IOCTL_CASTKMS_CAPTURE_ATTACH_MONITOR, &attach) < 0) {
		perror("DRM_IOCTL_CASTKMS_CAPTURE_ATTACH_MONITOR");
		return -1;
	}
	return 0;
}

static int detach_monitor(int fd, uint32_t connector_id)
{
	struct drm_castkms_capture_detach_monitor detach = {
		.connector_id = connector_id,
	};

	if (ioctl(fd, DRM_IOCTL_CASTKMS_CAPTURE_DETACH_MONITOR, &detach) < 0) {
		perror("DRM_IOCTL_CASTKMS_CAPTURE_DETACH_MONITOR");
		return -1;
	}
	return 0;
}

static int find_output_card(uint32_t output_index)
{
	char expected_prefix[64];
	int card = -1;

	snprintf(expected_prefix, sizeof(expected_prefix),
		 "CastKMS output %u audio:", output_index);
	while (snd_card_next(&card) == 0 && card >= 0) {
		char *longname = NULL;

		if (snd_card_get_longname(card, &longname) >= 0 && longname &&
		    !strncmp(longname, expected_prefix, strlen(expected_prefix))) {
			free(longname);
			return card;
		}
		free(longname);
	}
	return -1;
}

static int wait_for_output_card(uint32_t output_index)
{
	int card;

	for (unsigned int attempt = 0; attempt < 100; attempt++) {
		card = find_output_card(output_index);
		if (card >= 0)
			return card;
		usleep(10000);
	}
	return -1;
}

static int require_no_capture_pcm(int card)
{
	snd_pcm_info_t *info;
	snd_ctl_t *control = NULL;
	char control_name[32];
	int device = -1;
	int ret;

	snprintf(control_name, sizeof(control_name), "hw:%d", card);
	ret = snd_ctl_open(&control, control_name, SND_CTL_NONBLOCK);
	if (ret < 0) {
		fprintf(stderr, "snd_ctl_open(%s): %s\n",
			control_name, snd_strerror(ret));
		return -1;
	}
	snd_pcm_info_alloca(&info);
	for (;;) {
		ret = snd_ctl_pcm_next_device(control, &device);
		if (ret < 0 || device < 0)
			break;
		snd_pcm_info_set_device(info, (unsigned int)device);
		snd_pcm_info_set_subdevice(info, 0);
		snd_pcm_info_set_stream(info, SND_PCM_STREAM_CAPTURE);
		if (snd_ctl_pcm_info(control, info) >= 0) {
			fprintf(stderr,
				"CastKMS card unexpectedly exposes capture PCM %d\n",
				device);
			snd_ctl_close(control);
			return -1;
		}
	}
	if (ret < 0) {
		fprintf(stderr, "enumerate PCM devices: %s\n", snd_strerror(ret));
		snd_ctl_close(control);
		return -1;
	}
	snd_ctl_close(control);
	return 0;
}

static snd_pcm_t *open_playback(int card, snd_pcm_uframes_t *period,
				snd_pcm_uframes_t *buffer)
{
	snd_pcm_hw_params_t *hardware;
	snd_pcm_sw_params_t *software;
	snd_pcm_t *pcm = NULL;
	char pcm_name[32];
	unsigned int rate = TEST_RATE;
	int direction = 0;
	int ret;

	snprintf(pcm_name, sizeof(pcm_name), "hw:%d,0", card);
	ret = snd_pcm_open(&pcm, pcm_name, SND_PCM_STREAM_PLAYBACK,
			   SND_PCM_NONBLOCK);
	if (ret < 0)
		goto fail;
	snd_pcm_hw_params_alloca(&hardware);
	if ((ret = snd_pcm_hw_params_any(pcm, hardware)) < 0 ||
	    (ret = snd_pcm_hw_params_set_access(
		     pcm, hardware, SND_PCM_ACCESS_RW_INTERLEAVED)) < 0 ||
	    (ret = snd_pcm_hw_params_set_format(
		     pcm, hardware, SND_PCM_FORMAT_S16_LE)) < 0 ||
	    (ret = snd_pcm_hw_params_set_channels(
		     pcm, hardware, TEST_CHANNELS)) < 0 ||
	    (ret = snd_pcm_hw_params_set_rate(pcm, hardware, rate, 0)) < 0)
		goto fail;
	*period = TEST_PERIOD_FRAMES;
	ret = snd_pcm_hw_params_set_period_size_near(
		pcm, hardware, period, &direction);
	if (ret < 0)
		goto fail;
	*buffer = *period * TEST_BUFFER_PERIODS;
	ret = snd_pcm_hw_params_set_buffer_size_near(pcm, hardware, buffer);
	if (ret < 0 || (ret = snd_pcm_hw_params(pcm, hardware)) < 0)
		goto fail;
	if (rate != TEST_RATE || *period != TEST_PERIOD_FRAMES) {
		fprintf(stderr, "unexpected playback geometry: rate=%u period=%lu\n",
			rate, (unsigned long)*period);
		snd_pcm_close(pcm);
		return NULL;
	}
	snd_pcm_sw_params_alloca(&software);
	if ((ret = snd_pcm_sw_params_current(pcm, software)) < 0 ||
	    (ret = snd_pcm_sw_params_set_start_threshold(
		     pcm, software, *buffer)) < 0 ||
	    (ret = snd_pcm_sw_params_set_avail_min(
		     pcm, software, *period)) < 0 ||
	    (ret = snd_pcm_sw_params(pcm, software)) < 0)
		goto fail;
	return pcm;

fail:
	fprintf(stderr, "configure %s: %s\n", pcm_name, snd_strerror(ret));
	if (pcm)
		snd_pcm_close(pcm);
	return NULL;
}

static void fill_pattern(uint8_t *samples, size_t frames)
{
	static const uint8_t frame[TEST_FRAME_BYTES] = {
		0x45, 0x23, 0xbb, 0xdc,
	};

	for (size_t index = 0; index < frames; index++)
		memcpy(samples + index * TEST_FRAME_BYTES, frame, sizeof(frame));
}

static int write_frames(snd_pcm_t *pcm, const uint8_t *samples,
			snd_pcm_uframes_t frames)
{
	snd_pcm_uframes_t written = 0;

	while (written < frames) {
		snd_pcm_sframes_t ret = snd_pcm_writei(
			pcm, samples + written * TEST_FRAME_BYTES,
			frames - written);

		if (ret == -EINTR)
			continue;
		if (ret == -EAGAIN) {
			usleep(1000);
			continue;
		}
		if (ret < 0) {
			fprintf(stderr, "snd_pcm_writei: %s\n", snd_strerror(ret));
			return -1;
		}
		written += (snd_pcm_uframes_t)ret;
	}
	return 0;
}

static int start_pattern_playback(snd_pcm_t *pcm, uint8_t *samples,
				  snd_pcm_uframes_t buffer)
{
	int ret;

	fill_pattern(samples, buffer);
	ret = snd_pcm_prepare(pcm);
	if (ret < 0) {
		fprintf(stderr, "snd_pcm_prepare: %s\n", snd_strerror(ret));
		return -1;
	}
	if (write_frames(pcm, samples, buffer))
		return -1;
	if (snd_pcm_state(pcm) == SND_PCM_STATE_PREPARED) {
		ret = snd_pcm_start(pcm);
		if (ret < 0) {
			fprintf(stderr, "snd_pcm_start: %s\n", snd_strerror(ret));
			return -1;
		}
	}
	if (snd_pcm_state(pcm) != SND_PCM_STATE_RUNNING) {
		fprintf(stderr, "playback did not enter RUNNING state\n");
		return -1;
	}
	return 0;
}

static int open_audio_tap(int grant_fd, uint32_t connector_id,
			  struct drm_castkms_open_audio_tap *tap)
{
	*tap = (struct drm_castkms_open_audio_tap) {
		.connector_id = connector_id,
		.fd = -1,
		.fd_flags = O_NONBLOCK,
	};
	if (ioctl(grant_fd, DRM_IOCTL_CASTKMS_OPEN_AUDIO_TAP, tap) < 0) {
		perror("DRM_IOCTL_CASTKMS_OPEN_AUDIO_TAP");
		return -1;
	}
	if (tap->fd < 0 || tap->format != DRM_CASTKMS_AUDIO_FORMAT_S16_LE ||
	    tap->rate != TEST_RATE || tap->channels != TEST_CHANNELS ||
	    tap->frame_bytes != TEST_FRAME_BYTES || !tap->buffer_frames ||
	    tap->reserved || !(fcntl(tap->fd, F_GETFD) & FD_CLOEXEC) ||
	    !(fcntl(tap->fd, F_GETFL) & O_NONBLOCK)) {
		fprintf(stderr, "OPEN_AUDIO_TAP returned invalid metadata\n");
		close(tap->fd);
		tap->fd = -1;
		return -1;
	}
	errno = 0;
	if (lseek(tap->fd, 0, SEEK_CUR) != (off_t)-1 || errno != ESPIPE) {
		fprintf(stderr, "audio tap descriptor is unexpectedly seekable\n");
		close(tap->fd);
		tap->fd = -1;
		return -1;
	}
	return 0;
}

static int require_exclusive_tap(int grant_fd, uint32_t connector_id)
{
	struct drm_castkms_open_audio_tap second = {
		.connector_id = connector_id,
		.fd = -1,
		.fd_flags = O_NONBLOCK,
	};

	errno = 0;
	if (ioctl(grant_fd, DRM_IOCTL_CASTKMS_OPEN_AUDIO_TAP, &second) == 0) {
		fprintf(stderr, "a second audio tap unexpectedly opened\n");
		close(second.fd);
		return -1;
	}
	if (errno != EBUSY || second.fd != -1) {
		fprintf(stderr, "second audio tap failed with %s instead of EBUSY\n",
			strerror(errno));
		return -1;
	}
	return 0;
}

static int analyze_pattern(const uint8_t *samples, size_t bytes,
			   uint64_t *matching_frames,
			   uint64_t *silent_frames,
			   uint64_t *unexpected_frames)
{
	static const uint8_t pattern[TEST_FRAME_BYTES] = {
		0x45, 0x23, 0xbb, 0xdc,
	};
	static const uint8_t silence[TEST_FRAME_BYTES] = { 0 };

	if (bytes % TEST_FRAME_BYTES)
		return -1;
	for (size_t offset = 0; offset < bytes; offset += TEST_FRAME_BYTES) {
		if (!memcmp(samples + offset, pattern, sizeof(pattern)))
			(*matching_frames)++;
		else if (!memcmp(samples + offset, silence, sizeof(silence)))
			(*silent_frames)++;
		else
			(*unexpected_frames)++;
	}
	return 0;
}

static int capture_pattern(snd_pcm_t *pcm, int tap_fd,
			   const uint8_t *period_samples,
			   snd_pcm_uframes_t period)
{
	uint8_t captured[64 * 1024];
	uint64_t matching_frames = 0;
	uint64_t silent_frames = 0;
	uint64_t unexpected_frames = 0;
	struct pollfd poll_fd = {
		.fd = tap_fd,
		.events = POLLIN | POLLHUP | POLLERR,
	};
	int64_t start = monotonic_ns();

	if (start < 0)
		return -1;
	while (monotonic_ns() - start < 1000000000LL) {
		snd_pcm_sframes_t available = snd_pcm_avail_update(pcm);

		if (available == -EPIPE) {
			fprintf(stderr, "playback underrun during tap test\n");
			return -1;
		}
		if (available < 0) {
			fprintf(stderr, "snd_pcm_avail_update: %s\n",
				snd_strerror((int)available));
			return -1;
		}
		while (available >= (snd_pcm_sframes_t)period) {
			if (write_frames(pcm, period_samples, period))
				return -1;
			available -= period;
		}

		poll_fd.revents = 0;
		if (poll(&poll_fd, 1, 25) < 0) {
			if (errno == EINTR)
				continue;
			perror("poll audio tap");
			return -1;
		}
		if (poll_fd.revents & (POLLHUP | POLLERR | POLLNVAL)) {
			fprintf(stderr, "audio tap terminated during playback\n");
			return -1;
		}
		for (;;) {
			ssize_t count = read(tap_fd, captured, sizeof(captured));

			if (count > 0) {
				if (analyze_pattern(captured, (size_t)count,
						    &matching_frames, &silent_frames,
						    &unexpected_frames)) {
					fprintf(stderr, "tap returned a partial frame\n");
					return -1;
				}
				continue;
			}
			if (count < 0 && errno == EINTR)
				continue;
			if (count < 0 && errno == EAGAIN)
				break;
			fprintf(stderr, "read audio tap: %s\n",
				count < 0 ? strerror(errno) : "end of file");
			return -1;
		}
	}

	printf("tap_matching_frames=%llu\n",
	       (unsigned long long)matching_frames);
	printf("tap_silent_frames=%llu\n",
	       (unsigned long long)silent_frames);
	printf("tap_unexpected_frames=%llu\n",
	       (unsigned long long)unexpected_frames);
	if (matching_frames < TEST_MIN_MATCHING_FRAMES || unexpected_frames) {
		fprintf(stderr, "audio tap did not preserve the playback pattern\n");
		return -1;
	}
	return 0;
}

static int drain_tap(int fd)
{
	uint8_t buffer[4096];

	for (;;) {
		ssize_t count = read(fd, buffer, sizeof(buffer));

		if (count > 0)
			continue;
		if (count < 0 && errno == EINTR)
			continue;
		if (count < 0 && errno == EAGAIN)
			return 0;
		return -1;
	}
}

static int require_idle_silence(int tap_fd)
{
	uint8_t samples[TEST_PERIOD_BYTES * 2];
	struct pollfd poll_fd = {
		.fd = tap_fd,
		.events = POLLIN | POLLHUP | POLLERR,
	};
	ssize_t count;

	if (drain_tap(tap_fd))
		return -1;
	usleep(30000);
	if (drain_tap(tap_fd))
		return -1;
	if (poll(&poll_fd, 1, 100) <= 0 ||
	    poll_fd.revents & (POLLHUP | POLLERR | POLLNVAL)) {
		fprintf(stderr, "idle audio tap did not become readable\n");
		return -1;
	}
	do {
		count = read(tap_fd, samples, sizeof(samples));
	} while (count < 0 && errno == EINTR);
	if (count <= 0 || (size_t)count % TEST_FRAME_BYTES) {
		fprintf(stderr, "read idle audio tap: %s\n",
			count < 0 ? strerror(errno) : "no complete frames");
		return -1;
	}
	for (ssize_t index = 0; index < count; index++) {
		if (samples[index]) {
			fprintf(stderr, "idle audio tap returned non-silent data\n");
			return -1;
		}
	}
	return 0;
}

static int require_terminal_detach(int grant_fd, uint32_t connector_id,
				   int tap_fd)
{
	struct pollfd poll_fd = {
		.fd = tap_fd,
		.events = POLLIN | POLLHUP | POLLERR,
	};
	uint8_t frame[TEST_FRAME_BYTES];
	ssize_t count;

	if (detach_monitor(grant_fd, connector_id))
		return -1;
	if (poll(&poll_fd, 1, 1000) <= 0 ||
	    !(poll_fd.revents & (POLLHUP | POLLERR))) {
		fprintf(stderr, "audio tap did not report terminal detach\n");
		return -1;
	}
	errno = 0;
	count = read(tap_fd, frame, sizeof(frame));
	if (count >= 0 || errno != ENOTCONN) {
		fprintf(stderr, "detached audio tap read returned %s\n",
			count < 0 ? strerror(errno) : "data");
		return -1;
	}
	return 0;
}

static void usage(const char *program)
{
	fprintf(stderr,
		"usage: %s\n"
		"  Requires an attachment/audio grant in CASTKMS_GRANT_FD.\n",
		program);
}

int main(int argc, char **argv)
{
	struct drm_castkms_open_audio_tap tap = { .fd = -1 };
	struct drm_castkms_get_grant grant = { 0 };
	snd_pcm_uframes_t period = 0;
	snd_pcm_uframes_t buffer = 0;
	uint8_t *playback_samples = NULL;
	snd_pcm_t *pcm = NULL;
	bool attached = false;
	bool detached = false;
	int grant_fd = -1;
	int card;
	int result = EXIT_FAILURE;

	if (argc == 2 && (!strcmp(argv[1], "-h") ||
			  !strcmp(argv[1], "--help"))) {
		usage(argv[0]);
		return EXIT_SUCCESS;
	}
	if (argc != 1) {
		usage(argv[0]);
		return EXIT_FAILURE;
	}
	grant_fd = open_grant(&grant);
	if (grant_fd < 0)
		goto out;
	if (attach_monitor(grant_fd, grant.connector_id))
		goto out;
	attached = true;
	card = wait_for_output_card(grant.output_index);
	if (card < 0) {
		fprintf(stderr, "CastKMS attachment audio card did not appear\n");
		goto out;
	}
	printf("audio_card_index=%d\n", card);
	if (require_no_capture_pcm(card))
		goto out;
	printf("alsa_capture_pcm_absent=pass\n");
	pcm = open_playback(card, &period, &buffer);
	if (!pcm)
		goto out;
	playback_samples = malloc(buffer * TEST_FRAME_BYTES);
	if (!playback_samples) {
		perror("allocate playback buffer");
		goto out;
	}
	if (start_pattern_playback(pcm, playback_samples, buffer))
		goto out;
	printf("playback_before_tap=pass\n");
	if (open_audio_tap(grant_fd, grant.connector_id, &tap))
		goto out;
	printf("audio_tap_metadata=pass\n");
	if (require_exclusive_tap(grant_fd, grant.connector_id))
		goto out;
	printf("audio_tap_exclusive=pass\n");
	if (capture_pattern(pcm, tap.fd, playback_samples, period))
		goto out;
	printf("audio_tap_playback_integrity=pass\n");
	if (snd_pcm_drop(pcm) < 0) {
		fprintf(stderr, "snd_pcm_drop failed\n");
		goto out;
	}
	if (require_idle_silence(tap.fd))
		goto out;
	printf("audio_tap_idle_silence=pass\n");
	snd_pcm_close(pcm);
	pcm = NULL;
	if (require_terminal_detach(grant_fd, grant.connector_id, tap.fd))
		goto out;
	detached = true;
	printf("audio_tap_detach_lifetime=pass\n");
	printf("audio_tap=pass\n");
	result = EXIT_SUCCESS;

out:
	if (pcm)
		snd_pcm_close(pcm);
	if (tap.fd >= 0)
		close(tap.fd);
	if (attached && !detached)
		detach_monitor(grant_fd, grant.connector_id);
	if (grant_fd >= 0)
		close(grant_fd);
	free(playback_samples);
	return result;
}
