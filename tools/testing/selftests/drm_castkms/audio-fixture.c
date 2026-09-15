// SPDX-License-Identifier: GPL-2.0-only
#include "audio-fixture.h"
#include <fcntl.h>
#include <poll.h>
#include <sound/asound.h>
#include <string.h>
#include <sys/ioctl.h>
#include <unistd.h>

void audio_edid(unsigned char bytes[256])
{
	const unsigned char header[] = { 0, 255, 255, 255, 255, 255, 255, 0 };
	const unsigned char description[] = { 1, 3, 0x80, 52, 29, 120, 0x0a };
	const unsigned char cta[] = { 2, 3, 12, 0x40, 0x23, 0x09, 0x07, 0x01, 0x83, 1, 0, 0 };

	memset(bytes, 0, 256);
	memcpy(bytes, header, sizeof(header));
	bytes[8] = 0x31;
	bytes[9] = 0xd8;
	bytes[10] = 42;
	memcpy(bytes + 18, description, sizeof(description));
	memset(bytes + 38, 1, 16);
	bytes[57] = 0xfc;
	memcpy(bytes + 59, "CastKMS Audio", 13);
	bytes[126] = 1;
	memcpy(bytes + 128, cta, sizeof(cta));
	for (unsigned int block = 0; block < 256; block += 128) {
		unsigned char sum = 0;

		for (unsigned int i = 0; i < 127; i++)
			sum += bytes[block + i];
		bytes[block + 127] = -sum;
	}
}

int audio_card(unsigned int output)
{
	char path[64], id[32];

	snprintf(id, sizeof(id), "CastKMS%u", output);
	for (unsigned int i = 0; i < 32; i++) {
		struct snd_ctl_card_info info = { 0 };
		int fd, ret;

		snprintf(path, sizeof(path), "/dev/snd/controlC%u", i);
		fd = open(path, O_RDONLY | O_CLOEXEC);
		if (fd < 0)
			continue;
		ret = ioctl(fd, SNDRV_CTL_IOCTL_CARD_INFO, &info);
		close(fd);
		if (!ret && !strcmp((char *)info.driver, "castkms") &&
		    !strcmp((char *)info.id, id))
			return i;
	}
	return -1;
}

struct drm_castkms_audio_files audio_capture(int fd, uint32_t crtc, uint32_t connector)
{
	struct drm_castkms_audio_files files = { -1, -1 };
	struct drm_castkms_create_audio_capture create = {
		.crtc_id = crtc, .connector_id = connector, .files = (uintptr_t)&files,
		.flags = DRM_CASTKMS_AUDIO_NONBLOCK,
	};

	CHECK(ioctl(fd, DRM_IOCTL_CASTKMS_CREATE_AUDIO_CAPTURE, &create) == 0);
	CHECK(files.audio_fd >= 0 && files.revoke_fd >= 0 && files.audio_fd != files.revoke_fd);
	CHECK(fcntl(files.audio_fd, F_GETFD) == FD_CLOEXEC);
	CHECK(fcntl(files.revoke_fd, F_GETFD) == FD_CLOEXEC);
	CHECK((fcntl(files.audio_fd, F_GETFL) & O_ACCMODE) == O_RDONLY);
	return files;
}

void audio_terminal(int fd)
{
	struct pollfd event = { .fd = fd, .events = POLLIN };
	unsigned int sample;

	CHECK(poll(&event, 1, 1000) == 1);
	CHECK((event.revents & (POLLHUP | POLLERR)) == (POLLHUP | POLLERR));
	CHECK(!(event.revents & POLLIN));
	CHECK(read(fd, &sample, sizeof(sample)) < 0 && errno != EAGAIN);
}
