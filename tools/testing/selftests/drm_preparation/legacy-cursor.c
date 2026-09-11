// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <errno.h>
#include <fcntl.h>
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
	uint32_t crtc, connector, cursor, framebuffer, primary_handle, cursor_handle;
};

static uint64_t property(int fd, uint32_t plane, const char *name)
{
	drmModeObjectProperties *properties = drmModeObjectGetProperties(fd, plane,
								       DRM_MODE_OBJECT_PLANE);
	uint64_t value = 0;
	bool found = false;
	unsigned int i;

	if (!properties)
		ksft_exit_fail_msg("Cannot query plane properties: %m\n");
	for (i = 0; i < properties->count_props; i++) {
		drmModePropertyRes *p = drmModeGetProperty(fd, properties->props[i]);

		if (!p)
			ksft_exit_fail_msg("Cannot query property: %m\n");
		if (!strcmp(p->name, name)) {
			value = properties->prop_values[i];
			found = true;
		}
		drmModeFreeProperty(p);
	}
	drmModeFreeObjectProperties(properties);
	if (!found)
		ksft_exit_fail_msg("Missing %s property\n", name);
	return value;
}

static void setup(struct fixture *f, const char *path)
{
	drmVersionPtr version;
	drmModeRes *resources;
	drmModeConnector *connector;
	drmModePlaneRes *planes;
	struct drm_mode_create_dumb create = {};
	uint32_t handles[4] = {}, pitches[4] = {}, offsets[4] = {};
	uint64_t capability = 0;
	unsigned int i;

	f->fd = open(path, O_RDWR | O_CLOEXEC);
	if (f->fd < 0)
		ksft_exit_skip("Cannot open %s: %m\n", path);
	version = drmGetVersion(f->fd);
	if (!version || strcmp(version->name, "vkms"))
		ksft_exit_skip("Only run on an isolated VKMS device\n");
	drmFreeVersion(version);
	if (drmGetCap(f->fd, DRM_CAP_ATOMIC_PREPARATION, &capability) || !capability)
		ksft_exit_skip("VKMS preparation must be enabled\n");
	if (drmSetClientCap(f->fd, DRM_CLIENT_CAP_ATOMIC, 1) || drmSetMaster(f->fd))
		ksft_exit_skip("Cannot acquire master and inspect atomic properties: %m\n");
	resources = drmModeGetResources(f->fd);
	if (!resources || resources->count_crtcs != 1 || resources->count_connectors != 1)
		ksft_exit_skip("Requires a single-output VKMS fixture\n");
	f->crtc = resources->crtcs[0];
	f->connector = resources->connectors[0];
	drmModeFreeResources(resources);
	planes = drmModeGetPlaneResources(f->fd);
	if (!planes)
		ksft_exit_fail_msg("Cannot query planes: %m\n");
	for (i = 0; i < planes->count_planes; i++) {
		drmModePlane *plane = drmModeGetPlane(f->fd, planes->planes[i]);

		if (!plane)
			ksft_exit_fail_msg("Cannot query plane: %m\n");
		if ((plane->possible_crtcs & 1) &&
		    property(f->fd, plane->plane_id, "type") == DRM_PLANE_TYPE_CURSOR)
			f->cursor = plane->plane_id;
		drmModeFreePlane(plane);
	}
	drmModeFreePlaneResources(planes);
	if (!f->cursor)
		ksft_exit_skip("No universal cursor available\n");
	connector = drmModeGetConnector(f->fd, f->connector);
	if (!connector || !connector->count_modes)
		ksft_exit_skip("No display mode available\n");
	create.width = connector->modes[0].hdisplay;
	create.height = connector->modes[0].vdisplay;
	create.bpp = 32;
	if (drmIoctl(f->fd, DRM_IOCTL_MODE_CREATE_DUMB, &create))
		ksft_exit_fail_msg("Cannot allocate primary buffer: %m\n");
	f->primary_handle = handles[0] = create.handle;
	pitches[0] = create.pitch;
	if (drmModeAddFB2(f->fd, create.width, create.height, DRM_FORMAT_XRGB8888,
			  handles, pitches, offsets, &f->framebuffer, 0) ||
	    drmModeSetCrtc(f->fd, f->crtc, f->framebuffer, 0, 0,
			   &f->connector, 1, &connector->modes[0]))
		ksft_exit_fail_msg("Cannot enable output: %m\n");
	drmModeFreeConnector(connector);
	create = (struct drm_mode_create_dumb) { .width = 64, .height = 64, .bpp = 32 };
	if (drmIoctl(f->fd, DRM_IOCTL_MODE_CREATE_DUMB, &create))
		ksft_exit_fail_msg("Cannot allocate cursor buffer: %m\n");
	f->cursor_handle = create.handle;
	if (create.pitch != 256)
		ksft_exit_fail_msg("VKMS cursor buffer must have the implicit cursor pitch\n");
}

static bool position_is(struct fixture *f, int32_t x, int32_t y)
{
	return (int32_t)property(f->fd, f->cursor, "CRTC_X") == x &&
	       (int32_t)property(f->fd, f->cursor, "CRTC_Y") == y;
}

static void show_after_hidden_move(struct fixture *f)
{
	int move = drmModeMoveCursor(f->fd, f->crtc, 13, 17);
	int show = drmModeSetCursor2(f->fd, f->crtc, f->cursor_handle, 64, 64, 0, 0);

	ksft_test_result(!move && !show && property(f->fd, f->cursor, "FB_ID") &&
			position_is(f, 13, 17), "An image appears at the remembered hidden position\n");
}

int main(int argc, char **argv)
{
	struct fixture f = {};
	struct drm_mode_destroy_dumb destroy;

	ksft_print_header();
	if (argc > 2)
		ksft_exit_fail_msg("Usage: %s [DEVICE]\n", argv[0]);
	setup(&f, argc > 1 ? argv[1] : "/dev/dri/card0");
	ksft_set_plan(1);
	show_after_hidden_move(&f);
	if (drmModeSetCursor(f.fd, f.crtc, 0, 0, 0))
		ksft_exit_fail_msg("Cannot hide cursor during cleanup: %m\n");
	if (drmModeSetCrtc(f.fd, f.crtc, 0, 0, 0, NULL, 0, NULL))
		ksft_exit_fail_msg("Cannot disable output: %m\n");
	drmModeRmFB(f.fd, f.framebuffer);
	destroy = (struct drm_mode_destroy_dumb) { .handle = f.primary_handle };
	drmIoctl(f.fd, DRM_IOCTL_MODE_DESTROY_DUMB, &destroy);
	if (f.cursor_handle) {
		destroy.handle = f.cursor_handle;
		drmIoctl(f.fd, DRM_IOCTL_MODE_DESTROY_DUMB, &destroy);
	}
	close(f.fd);
	ksft_finished();
}
