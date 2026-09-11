// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <dirent.h>
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
	uint32_t crtc, connector, plane, mode, framebuffer, handle;
	drmModeModeInfo timing;
};

static uint32_t property(int fd, uint32_t object, uint32_t type,
			 const char *name, uint64_t *value)
{
	drmModeObjectProperties *properties = drmModeObjectGetProperties(fd, object, type);
	uint32_t id = 0;
	unsigned int i;

	if (!properties)
		ksft_exit_fail_msg("Cannot query properties: %m\n");
	for (i = 0; i < properties->count_props; i++) {
		drmModePropertyRes *p = drmModeGetProperty(fd, properties->props[i]);

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

static void add(struct fixture *f, drmModeAtomicReq *request, uint32_t object,
		uint32_t type, const char *name, uint64_t value)
{
	uint32_t id = property(f->fd, object, type, name, NULL);

	if (drmModeAtomicAddProperty(request, object, id, value) < 0)
		ksft_exit_fail_msg("Cannot add %s\n", name);
}

static drmModeAtomicReq *new_request(void)
{
	drmModeAtomicReq *request = drmModeAtomicAlloc();

	if (!request)
		ksft_exit_fail_msg("Cannot allocate request\n");
	return request;
}

static void modeset(struct fixture *f, bool active)
{
	drmModeAtomicReq *request = new_request();

	add(f, request, f->crtc, DRM_MODE_OBJECT_CRTC, "ACTIVE", active);
	add(f, request, f->crtc, DRM_MODE_OBJECT_CRTC, "MODE_ID", active ? f->mode : 0);
	add(f, request, f->connector, DRM_MODE_OBJECT_CONNECTOR, "CRTC_ID", active ? f->crtc : 0);
	add(f, request, f->plane, DRM_MODE_OBJECT_PLANE, "CRTC_ID", active ? f->crtc : 0);
	add(f, request, f->plane, DRM_MODE_OBJECT_PLANE, "FB_ID", active ? f->framebuffer : 0);
	if (active) {
		add(f, request, f->plane, DRM_MODE_OBJECT_PLANE, "CRTC_X", 0);
		add(f, request, f->plane, DRM_MODE_OBJECT_PLANE, "CRTC_Y", 0);
		add(f, request, f->plane, DRM_MODE_OBJECT_PLANE, "CRTC_W", f->timing.hdisplay);
		add(f, request, f->plane, DRM_MODE_OBJECT_PLANE, "CRTC_H", f->timing.vdisplay);
		add(f, request, f->plane, DRM_MODE_OBJECT_PLANE, "SRC_X", 0);
		add(f, request, f->plane, DRM_MODE_OBJECT_PLANE, "SRC_Y", 0);
		add(f, request, f->plane, DRM_MODE_OBJECT_PLANE, "SRC_W", f->timing.hdisplay << 16);
		add(f, request, f->plane, DRM_MODE_OBJECT_PLANE, "SRC_H", f->timing.vdisplay << 16);
	}
	if (drmModeAtomicCommit(f->fd, request, DRM_MODE_ATOMIC_ALLOW_MODESET, NULL))
		ksft_exit_fail_msg("Cannot set output state: %m\n");
	drmModeAtomicFree(request);
}

static void setup(struct fixture *f, const char *path, bool preparation_client)
{
	drmVersionPtr version;
	drmModeRes *resources;
	drmModeConnector *connector;
	drmModePlaneRes *planes;
	struct drm_mode_create_dumb create = {};
	uint32_t handles[4] = {}, strides[4] = {}, offsets[4] = {};
	unsigned int i;

	f->fd = open(path, O_RDWR | O_CLOEXEC);
	if (f->fd < 0)
		ksft_exit_skip("Cannot open %s: %m\n", path);
	version = drmGetVersion(f->fd);
	if (!version || strcmp(version->name, "vkms"))
		ksft_exit_skip("Only run on an isolated VKMS device\n");
	drmFreeVersion(version);
	if (drmSetClientCap(f->fd, DRM_CLIENT_CAP_ATOMIC, 1) || drmSetMaster(f->fd))
		ksft_exit_skip("Cannot become an atomic modesetting client: %m\n");
	if (preparation_client && drmSetClientCap(f->fd, DRM_CLIENT_CAP_ATOMIC_PREPARATION, 1))
		ksft_exit_skip("Cannot enable explicit preparation support: %m\n");
	resources = drmModeGetResources(f->fd);
	if (!resources || resources->count_crtcs != 1 || resources->count_connectors != 1)
		ksft_exit_skip("Requires a single-output VKMS fixture\n");
	f->crtc = resources->crtcs[0];
	f->connector = resources->connectors[0];
	drmModeFreeResources(resources);
	connector = drmModeGetConnector(f->fd, f->connector);
	if (!connector || !connector->count_modes)
		ksft_exit_skip("No display mode available\n");
	f->timing = connector->modes[0];
	drmModeFreeConnector(connector);
	planes = drmModeGetPlaneResources(f->fd);
	if (!planes)
		ksft_exit_fail_msg("Cannot query planes: %m\n");
	for (i = 0; i < planes->count_planes; i++) {
		drmModePlane *plane = drmModeGetPlane(f->fd, planes->planes[i]);
		uint64_t type;

		if (!plane)
			ksft_exit_fail_msg("Cannot query plane: %m\n");
		property(f->fd, plane->plane_id, DRM_MODE_OBJECT_PLANE, "type", &type);
		if ((plane->possible_crtcs & 1) && type == DRM_PLANE_TYPE_PRIMARY)
			f->plane = plane->plane_id;
		drmModeFreePlane(plane);
	}
	drmModeFreePlaneResources(planes);
	if (!f->plane)
		ksft_exit_skip("No primary plane available\n");
	create.width = f->timing.hdisplay;
	create.height = f->timing.vdisplay;
	create.bpp = 32;
	if (drmIoctl(f->fd, DRM_IOCTL_MODE_CREATE_DUMB, &create))
		ksft_exit_fail_msg("Cannot allocate framebuffer: %m\n");
	f->handle = handles[0] = create.handle;
	strides[0] = create.pitch;
	if (drmModeAddFB2(f->fd, create.width, create.height, DRM_FORMAT_XRGB8888,
			  handles, strides, offsets, &f->framebuffer, 0) ||
	    drmModeCreatePropertyBlob(f->fd, &f->timing, sizeof(f->timing), &f->mode))
		ksft_exit_fail_msg("Cannot register framebuffer and mode: %m\n");
	modeset(f, true);
}

static int descriptor_count(void)
{
	DIR *directory = opendir("/proc/self/fd");
	struct dirent *entry;
	int count = 0;

	if (!directory)
		ksft_exit_fail_msg("Cannot inspect descriptors: %m\n");
	while ((entry = readdir(directory)))
		if (entry->d_name[0] != '.')
			count++;
	closedir(directory);
	return count;
}

static void output_fence(struct fixture *f)
{
	drmModeAtomicReq *request = new_request();
	int fence = -1, ret;
	struct pollfd pollfd;

	add(f, request, f->crtc, DRM_MODE_OBJECT_CRTC, "OUT_FENCE_PTR", (uintptr_t)&fence);
	ret = drmModeAtomicCommit(f->fd, request, 0, NULL);
	pollfd = (struct pollfd) { .fd = fence, .events = POLLIN };
	ksft_test_result(!ret && fence >= 0 && (fcntl(fence, F_GETFD) & FD_CLOEXEC) &&
			poll(&pollfd, 1, 3000) == 1 && (pollfd.revents & POLLIN),
			"Accepted output fence is installed, close-on-exec and completes\n");
	if (fence >= 0)
		close(fence);
	drmModeAtomicFree(request);
}

static void failed_update(struct fixture *f)
{
	drmModeAtomicReq *request = new_request();
	int before = descriptor_count(), fence = -1, ret;

	add(f, request, f->crtc, DRM_MODE_OBJECT_CRTC, "OUT_FENCE_PTR", (uintptr_t)&fence);
	add(f, request, f->plane, DRM_MODE_OBJECT_PLANE, "SRC_W", 0);
	ret = drmModeAtomicCommit(f->fd, request, 0, NULL);
	ksft_test_result(ret < 0 && fence == -1 && descriptor_count() == before,
			"Rejected geometry returns no fence and leaks no descriptor\n");
	drmModeAtomicFree(request);
}

static void test_only(struct fixture *f)
{
	drmModeAtomicReq *request = new_request();
	int before = descriptor_count(), fence = 77, ret;

	add(f, request, f->crtc, DRM_MODE_OBJECT_CRTC, "OUT_FENCE_PTR", (uintptr_t)&fence);
	ret = drmModeAtomicCommit(f->fd, request, DRM_MODE_ATOMIC_TEST_ONLY, NULL);
	ksft_test_result(!ret && fence == -1 && descriptor_count() == before,
			"Test-only update initializes output without installing a fence\n");
	drmModeAtomicFree(request);
}

static void inactive_output(struct fixture *f)
{
	drmModeAtomicReq *request = new_request();
	int fence = 77, ret;

	modeset(f, false);
	add(f, request, f->crtc, DRM_MODE_OBJECT_CRTC, "OUT_FENCE_PTR", (uintptr_t)&fence);
	errno = 0;
	ret = drmModeAtomicCommit(f->fd, request, 0, NULL);
	ksft_test_result(ret < 0 && errno == EINVAL && fence == -1,
			"Inactive output rejects a completion fence\n");
	drmModeAtomicFree(request);
}

int main(int argc, char **argv)
{
	struct fixture f = {};
	struct drm_mode_destroy_dumb destroy;

	ksft_print_header();
	if (argc > 3 || (argc == 3 && strcmp(argv[2], "--preparation-client")))
		ksft_exit_fail_msg("Usage: %s [DEVICE [--preparation-client]]\n", argv[0]);
	setup(&f, argc > 1 ? argv[1] : "/dev/dri/card0", argc == 3);
	ksft_set_plan(4);
	output_fence(&f);
	failed_update(&f);
	test_only(&f);
	inactive_output(&f);
	drmModeRmFB(f.fd, f.framebuffer);
	drmModeDestroyPropertyBlob(f.fd, f.mode);
	destroy = (struct drm_mode_destroy_dumb) { .handle = f.handle };
	drmIoctl(f.fd, DRM_IOCTL_MODE_DESTROY_DUMB, &destroy);
	close(f.fd);
	ksft_finished();
}
