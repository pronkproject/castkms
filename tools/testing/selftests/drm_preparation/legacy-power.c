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
	uint32_t crtc, connector, framebuffer, handle, dpms;
};

static uint32_t property(struct fixture *f, uint32_t object, uint32_t type,
			 const char *name, uint64_t *value)
{
	drmModeObjectProperties *properties = drmModeObjectGetProperties(f->fd, object, type);
	uint32_t id = 0;
	unsigned int i;

	if (!properties)
		ksft_exit_fail_msg("Cannot query properties: %m\n");
	for (i = 0; i < properties->count_props; i++) {
		drmModePropertyRes *p = drmModeGetProperty(f->fd, properties->props[i]);

		if (!p)
			ksft_exit_fail_msg("Cannot query property: %m\n");
		if (!strcmp(p->name, name)) {
			id = p->prop_id;
			if (value)
				*value = properties->prop_values[i];
		}
		drmModeFreeProperty(p);
	}
	drmModeFreeObjectProperties(properties);
	if (!id)
		ksft_exit_fail_msg("Missing %s property\n", name);
	return id;
}

static void setup(struct fixture *f, const char *path)
{
	drmVersionPtr version;
	drmModeRes *resources;
	drmModeConnector *connector;
	struct drm_mode_create_dumb create = {};
	uint32_t handles[4] = {}, pitches[4] = {}, offsets[4] = {};
	uint64_t capability = 0;

	f->fd = open(path, O_RDWR | O_CLOEXEC);
	if (f->fd < 0)
		ksft_exit_skip("Cannot open %s: %m\n", path);
	version = drmGetVersion(f->fd);
	if (!version || strcmp(version->name, "vkms"))
		ksft_exit_skip("Only run on an isolated VKMS device\n");
	drmFreeVersion(version);
	if (drmGetCap(f->fd, DRM_CAP_ATOMIC_PREPARATION, &capability) || !capability)
		ksft_exit_skip("VKMS preparation must be enabled\n");
	if (drmSetMaster(f->fd))
		ksft_exit_skip("Cannot acquire display ownership: %m\n");
	resources = drmModeGetResources(f->fd);
	if (!resources || resources->count_crtcs != 1 || resources->count_connectors != 1)
		ksft_exit_skip("Requires a single-output VKMS fixture\n");
	f->crtc = resources->crtcs[0];
	f->connector = resources->connectors[0];
	drmModeFreeResources(resources);
	f->dpms = property(f, f->connector, DRM_MODE_OBJECT_CONNECTOR, "DPMS", NULL);
	connector = drmModeGetConnector(f->fd, f->connector);
	if (!connector || !connector->count_modes)
		ksft_exit_skip("No display mode available\n");
	create.width = connector->modes[0].hdisplay;
	create.height = connector->modes[0].vdisplay;
	create.bpp = 32;
	if (drmIoctl(f->fd, DRM_IOCTL_MODE_CREATE_DUMB, &create))
		ksft_exit_fail_msg("Cannot allocate primary buffer: %m\n");
	f->handle = handles[0] = create.handle;
	pitches[0] = create.pitch;
	if (drmModeAddFB2(f->fd, create.width, create.height, DRM_FORMAT_XRGB8888,
			  handles, pitches, offsets, &f->framebuffer, 0) ||
	    drmModeSetCrtc(f->fd, f->crtc, f->framebuffer, 0, 0,
			   &f->connector, 1, &connector->modes[0]))
		ksft_exit_fail_msg("Cannot enable output: %m\n");
	drmModeFreeConnector(connector);
}

static void change_power(struct fixture *f, uint64_t mode, bool on)
{
	uint64_t preference, active, routing;
	int ret = drmModeObjectSetProperty(f->fd, f->connector, DRM_MODE_OBJECT_CONNECTOR,
					   f->dpms, mode);

	/* Inspect atomic properties only after issuing the legacy command. */
	if (drmSetClientCap(f->fd, DRM_CLIENT_CAP_ATOMIC, 1))
		ksft_exit_fail_msg("Cannot inspect atomic state: %m\n");
	property(f, f->connector, DRM_MODE_OBJECT_CONNECTOR, "DPMS", &preference);
	property(f, f->connector, DRM_MODE_OBJECT_CONNECTOR, "CRTC_ID", &routing);
	property(f, f->crtc, DRM_MODE_OBJECT_CRTC, "ACTIVE", &active);
	if (drmSetClientCap(f->fd, DRM_CLIENT_CAP_ATOMIC, 0))
		ksft_exit_fail_msg("Cannot restore legacy-only client: %m\n");
	ksft_test_result(!ret && active == on && routing == f->crtc &&
			preference == (on ? DRM_MODE_DPMS_ON : DRM_MODE_DPMS_OFF),
			"Power mode %llu changes activity without losing routing\n",
			(unsigned long long)mode);
}

int main(int argc, char **argv)
{
	struct fixture f = {};
	struct drm_mode_destroy_dumb destroy;

	ksft_print_header();
	if (argc > 2)
		ksft_exit_fail_msg("Usage: %s [DEVICE]\n", argv[0]);
	setup(&f, argc > 1 ? argv[1] : "/dev/dri/card0");
	ksft_set_plan(2);
	change_power(&f, DRM_MODE_DPMS_OFF, false);
	change_power(&f, DRM_MODE_DPMS_ON, true);
	if (drmModeSetCrtc(f.fd, f.crtc, 0, 0, 0, NULL, 0, NULL))
		ksft_exit_fail_msg("Cannot disable output: %m\n");
	drmModeRmFB(f.fd, f.framebuffer);
	destroy = (struct drm_mode_destroy_dumb) { .handle = f.handle };
	drmIoctl(f.fd, DRM_IOCTL_MODE_DESTROY_DUMB, &destroy);
	close(f.fd);
	ksft_finished();
}
