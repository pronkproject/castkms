// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdbool.h>
#include <stdint.h>
#include <string.h>
#include <unistd.h>
#include <drm_fourcc.h>
#include <xf86drm.h>
#include <xf86drmMode.h>
#include "../../../../include/uapi/drm/drm_prepare.h"
#include "../kselftest.h"

struct fixture {
	int fd;
	uint32_t crtc, connector, fb[2], handle[2];
	drmModeModeInfo mode;
};

static void setup(struct fixture *f, const char *path)
{
	drmVersionPtr version;
	drmModeRes *resources;
	drmModeConnector *connector;
	uint64_t capability = 0;
	unsigned int i;

	f->fd = open(path, O_RDWR | O_CLOEXEC | O_NONBLOCK);
	if (f->fd < 0)
		ksft_exit_skip("Cannot open %s: %m\n", path);
	version = drmGetVersion(f->fd);
	if (!version || strcmp(version->name, "vkms"))
		ksft_exit_skip("Only run on an isolated VKMS device\n");
	drmFreeVersion(version);
	if (drmGetCap(f->fd, DRM_CAP_ATOMIC_PREPARATION, &capability) || !capability)
		ksft_exit_skip("VKMS preparation must be enabled\n");
	if (drmSetMaster(f->fd))
		ksft_exit_skip("Cannot acquire master: %m\n");
	resources = drmModeGetResources(f->fd);
	if (!resources || resources->count_crtcs != 1 || resources->count_connectors != 1)
		ksft_exit_skip("Requires a single-output VKMS fixture\n");
	f->crtc = resources->crtcs[0];
	f->connector = resources->connectors[0];
	drmModeFreeResources(resources);
	connector = drmModeGetConnector(f->fd, f->connector);
	if (!connector || !connector->count_modes)
		ksft_exit_skip("No display mode available\n");
	f->mode = connector->modes[0];
	drmModeFreeConnector(connector);
	for (i = 0; i < 2; i++) {
		struct drm_mode_create_dumb create = {
			.width = f->mode.hdisplay, .height = f->mode.vdisplay, .bpp = 32,
		};
		uint32_t handles[4] = {}, pitches[4] = {}, offsets[4] = {};

		if (drmIoctl(f->fd, DRM_IOCTL_MODE_CREATE_DUMB, &create))
			ksft_exit_fail_msg("Cannot allocate image: %m\n");
		f->handle[i] = handles[0] = create.handle;
		pitches[0] = create.pitch;
		if (drmModeAddFB2(f->fd, create.width, create.height, DRM_FORMAT_XRGB8888,
				  handles, pitches, offsets, &f->fb[i], 0))
			ksft_exit_fail_msg("Cannot register image: %m\n");
	}
	if (drmModeSetCrtc(f->fd, f->crtc, f->fb[0], 0, 0, &f->connector, 1, &f->mode))
		ksft_exit_fail_msg("Cannot enable output: %m\n");
}

static bool event_is(struct fixture *f, uint64_t cookie)
{
	struct pollfd pollfd = { .fd = f->fd, .events = POLLIN };
	struct drm_event_vblank event;

	if (poll(&pollfd, 1, 2000) != 1 || pollfd.revents != POLLIN ||
	    read(f->fd, &event, sizeof(event)) != sizeof(event))
		return false;
	return event.base.type == DRM_EVENT_FLIP_COMPLETE &&
	       event.base.length == sizeof(event) && event.crtc_id == f->crtc &&
	       event.user_data == cookie && poll(&pollfd, 1, 0) == 0;
}

static bool image_is(struct fixture *f, uint32_t fb)
{
	drmModeCrtc *crtc = drmModeGetCrtc(f->fd, f->crtc);
	bool matches = crtc && crtc->buffer_id == fb;

	drmModeFreeCrtc(crtc);
	return matches;
}

static void flip_with_event(struct fixture *f)
{
	int ret = drmModePageFlip(f->fd, f->crtc, f->fb[1], DRM_MODE_PAGE_FLIP_EVENT,
				  (void *)(uintptr_t)0x12345678);

	ksft_test_result(!ret && event_is(f, 0x12345678) && image_is(f, f->fb[1]),
			"A legacy flip delivers one event without atomic negotiation\n");
}

static void invalid_image_has_no_event(struct fixture *f)
{
	struct pollfd pollfd = { .fd = f->fd, .events = POLLIN };
	int ret = drmModePageFlip(f->fd, f->crtc, UINT32_MAX, DRM_MODE_PAGE_FLIP_EVENT, NULL);

	ksft_test_result(ret < 0 && errno == ENOENT && image_is(f, f->fb[1]) &&
			poll(&pollfd, 1, 0) == 0, "An invalid image leaves no event or display change\n");
}

int main(int argc, char **argv)
{
	struct fixture f = {};
	unsigned int i;

	ksft_print_header();
	if (argc > 2)
		ksft_exit_fail_msg("Usage: %s [DEVICE]\n", argv[0]);
	setup(&f, argc > 1 ? argv[1] : "/dev/dri/card0");
	ksft_set_plan(2);
	flip_with_event(&f);
	invalid_image_has_no_event(&f);
	if (drmModeSetCrtc(f.fd, f.crtc, 0, 0, 0, NULL, 0, NULL))
		ksft_exit_fail_msg("Cannot disable output: %m\n");
	for (i = 0; i < 2; i++) {
		struct drm_mode_destroy_dumb destroy = { .handle = f.handle[i] };

		drmModeRmFB(f.fd, f.fb[i]);
		if (destroy.handle)
			drmIoctl(f.fd, DRM_IOCTL_MODE_DESTROY_DUMB, &destroy);
	}
	close(f.fd);
	ksft_finished();
}
