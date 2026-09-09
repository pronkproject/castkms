// SPDX-License-Identifier: GPL-2.0-only
/* Ordinary DRM lifecycle checks; private scene attribution is not observable here. */
#include <fcntl.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

#include "fixture.h"

struct display {
	uint32_t crtc, connector, plane, mode_blob;
	drmModeModeInfo mode;
};

static int open_client(const char *path)
{
	int fd = open(path, O_RDWR | O_CLOEXEC);
	drmVersion *version;

	CHECK(fd >= 0);
	version = drmGetVersion(fd);
	CHECK(version && !strcmp(version->name, "castkms"));
	CHECK(version->version_major == 0 && version->version_minor == 0);
	drmFreeVersion(version);
	CHECK(drmSetClientCap(fd, DRM_CLIENT_CAP_ATOMIC, 1) == 0);
	return fd;
}

static struct display discover(int fd)
{
	drmModeRes *resources = drmModeGetResources(fd);
	drmModePlaneRes *planes = drmModeGetPlaneResources(fd);
	drmModeConnector *connector;
	struct display d;

	CHECK(resources && resources->count_crtcs == 1 && resources->count_connectors == 1);
	CHECK(planes && planes->count_planes == 1);
	d.crtc = resources->crtcs[0];
	d.connector = resources->connectors[0];
	d.plane = planes->planes[0];
	connector = drmModeGetConnector(fd, d.connector);
	CHECK(connector && connector->connection == DRM_MODE_CONNECTED);
	CHECK(connector->count_modes > 0);
	d.mode = connector->modes[0];
	CHECK(drmModeCreatePropertyBlob(fd, &d.mode, sizeof(d.mode), &d.mode_blob) == 0);
	drmModeFreeConnector(connector);
	drmModeFreePlaneResources(planes);
	drmModeFreeResources(resources);
	return d;
}

static void enable(int fd, const struct display *d, uint32_t fb)
{
	drmModeAtomicReq *req = drmModeAtomicAlloc();

	CHECK(req);
	property(fd, req, d->connector, DRM_MODE_OBJECT_CONNECTOR, "CRTC_ID", d->crtc);
	property(fd, req, d->crtc, DRM_MODE_OBJECT_CRTC, "ACTIVE", 1);
	property(fd, req, d->crtc, DRM_MODE_OBJECT_CRTC, "MODE_ID", d->mode_blob);
	property(fd, req, d->plane, DRM_MODE_OBJECT_PLANE, "FB_ID", fb);
	property(fd, req, d->plane, DRM_MODE_OBJECT_PLANE, "CRTC_ID", d->crtc);
	property(fd, req, d->plane, DRM_MODE_OBJECT_PLANE, "SRC_X", 0);
	property(fd, req, d->plane, DRM_MODE_OBJECT_PLANE, "SRC_Y", 0);
	property(fd, req, d->plane, DRM_MODE_OBJECT_PLANE, "SRC_W", d->mode.hdisplay << 16);
	property(fd, req, d->plane, DRM_MODE_OBJECT_PLANE, "SRC_H", d->mode.vdisplay << 16);
	property(fd, req, d->plane, DRM_MODE_OBJECT_PLANE, "CRTC_X", 0);
	property(fd, req, d->plane, DRM_MODE_OBJECT_PLANE, "CRTC_Y", 0);
	property(fd, req, d->plane, DRM_MODE_OBJECT_PLANE, "CRTC_W", d->mode.hdisplay);
	property(fd, req, d->plane, DRM_MODE_OBJECT_PLANE, "CRTC_H", d->mode.vdisplay);
	CHECK(drmModeAtomicCommit(fd, req, DRM_MODE_ATOMIC_ALLOW_MODESET, NULL) == 0);
	drmModeAtomicFree(req);
}

static void expect_framebuffer(int fd, const struct display *d, uint32_t fb)
{
	drmModePlane *plane = drmModeGetPlane(fd, d->plane);

	CHECK(plane && plane->fb_id == fb);
	drmModeFreePlane(plane);
}

static int64_t monotonic_msec(void)
{
	struct timespec now;

	CHECK(clock_gettime(CLOCK_MONOTONIC, &now) == 0);
	return (int64_t)now.tv_sec * 1000 + now.tv_nsec / 1000000;
}

static void expect_gone(int fd, uint32_t fb)
{
	int64_t deadline = monotonic_msec() + 5000;

	/* Page-flip events precede the commit's final framebuffer reference release. */
	for (;;) {
		drmModeFB2 *info;

		errno = 0;
		info = drmModeGetFB2(fd, fb);
		if (!info) {
			CHECK(errno == ENOENT);
			return;
		}

		/* GETFB2 creates handles; aliased format planes share a returned handle. */
		for (unsigned int i = 0; i < 4; i++) {
			unsigned int j;

			if (!info->handles[i])
				continue;
			for (j = 0; j < i; j++)
				if (info->handles[j] == info->handles[i])
					break;
			if (j == i)
				CHECK(drmCloseBufferHandle(fd, info->handles[i]) == 0);
		}
		drmModeFreeFB2(info);
		CHECK(monotonic_msec() < deadline);
		usleep(1000);
	}
}

static void select_framebuffer(int fd, const struct display *d, uint32_t fb)
{
	flip(fd, d->plane, fb);
	expect_framebuffer(fd, d, fb);
}

static void unaccepted_replacement(int fd, const struct display *d,
				   uint32_t current, uint32_t candidate)
{
	drmModeAtomicReq *req = drmModeAtomicAlloc();

	CHECK(req);
	property(fd, req, d->plane, DRM_MODE_OBJECT_PLANE, "FB_ID", candidate);
	CHECK(drmModeAtomicCommit(fd, req, DRM_MODE_ATOMIC_TEST_ONLY, NULL) == 0);
	expect_framebuffer(fd, d, current);
	property(fd, req, d->plane, DRM_MODE_OBJECT_PLANE, "CRTC_W", d->mode.hdisplay / 2);
	errno = 0;
	CHECK(drmModeAtomicCommit(fd, req, 0, NULL) != 0 && errno == ERANGE);
	expect_framebuffer(fd, d, current);
	drmModeAtomicFree(req);
}

static void disable(int fd, const struct display *d)
{
	drmModeAtomicReq *req = drmModeAtomicAlloc();

	CHECK(req);
	property(fd, req, d->plane, DRM_MODE_OBJECT_PLANE, "FB_ID", 0);
	property(fd, req, d->plane, DRM_MODE_OBJECT_PLANE, "CRTC_ID", 0);
	property(fd, req, d->connector, DRM_MODE_OBJECT_CONNECTOR, "CRTC_ID", 0);
	property(fd, req, d->crtc, DRM_MODE_OBJECT_CRTC, "ACTIVE", 0);
	property(fd, req, d->crtc, DRM_MODE_OBJECT_CRTC, "MODE_ID", 0);
	CHECK(drmModeAtomicCommit(fd, req, DRM_MODE_ATOMIC_ALLOW_MODESET, NULL) == 0);
	drmModeAtomicFree(req);
	expect_framebuffer(fd, d, 0);
}

int main(int argc, char **argv)
{
	struct display d;
	struct buffer creator, associated, replacement, retained;
	drmModeAtomicReq *req;
	int first, second, peer, successor;

	if (argc != 2) {
		fprintf(stderr, "SKIP: supply a disposable Rust CastKMS DRM node\n");
		return 4;
	}
	first = open_client(argv[1]);
	CHECK(drmSetMaster(first) == 0);
	second = open_client(argv[1]);
	peer = open_client(argv[1]);
	CHECK(drmIsMaster(first) == 1);
	CHECK(drmIsMaster(second) == 0 && drmIsMaster(peer) == 0);
	d = discover(first);
	creator = create_buffer(first, d.mode.hdisplay, d.mode.vdisplay, 0x11);
	associated = create_buffer(peer, d.mode.hdisplay, d.mode.vdisplay, 0x22);
	enable(first, &d, creator.fb);

	req = drmModeAtomicAlloc();
	CHECK(req);
	property(peer, req, d.plane, DRM_MODE_OBJECT_PLANE, "FB_ID", associated.fb);
	errno = 0;
	CHECK(drmModeAtomicCommit(peer, req, 0, NULL) != 0 && errno == EACCES);
	drmModeAtomicFree(req);
	expect_framebuffer(first, &d, creator.fb);

	CHECK(drmDropMaster(first) == 0);
	CHECK(drmSetMaster(second) == 0);
	CHECK(drmIsMaster(first) == 0 && drmIsMaster(second) == 1);
	select_framebuffer(second, &d, creator.fb);
	unaccepted_replacement(second, &d, creator.fb, associated.fb);
	select_framebuffer(second, &d, associated.fb);
	select_framebuffer(second, &d, associated.fb);
	replacement = create_buffer(second, d.mode.hdisplay, d.mode.vdisplay, 0x33);
	select_framebuffer(second, &d, replacement.fb);

	CHECK(drmDropMaster(second) == 0);
	CHECK(drmSetMaster(first) == 0);
	select_framebuffer(first, &d, replacement.fb);
	select_framebuffer(first, &d, associated.fb);
	/* CLOSEFB drops the file's reference without disabling the active plane. */
	CHECK(drmModeCloseFB(peer, associated.fb) == 0);
	CHECK(close(peer) == 0);
	expect_framebuffer(first, &d, associated.fb);
	unaccepted_replacement(first, &d, associated.fb, replacement.fb);
	select_framebuffer(first, &d, replacement.fb);
	expect_gone(first, associated.fb);

	/* RMFB, unlike CLOSEFB, removes the active framebuffer from display state. */
	destroy_buffer(second, &replacement);
	expect_framebuffer(first, &d, 0);
	destroy_buffer(first, &creator);
	retained = create_buffer(second, d.mode.hdisplay, d.mode.vdisplay, 0x44);
	enable(first, &d, retained.fb);
	CHECK(drmModeCloseFB(second, retained.fb) == 0);
	CHECK(close(second) == 0);
	expect_framebuffer(first, &d, retained.fb);

	/* Keep a file open while the current master closes, then acquire a new master. */
	successor = open_client(argv[1]);
	CHECK(drmIsMaster(successor) == 0);
	CHECK(drmModeDestroyPropertyBlob(first, d.mode_blob) == 0);
	CHECK(close(first) == 0);
	CHECK(drmSetMaster(successor) == 0);
	expect_framebuffer(successor, &d, retained.fb);
	select_framebuffer(successor, &d, retained.fb);
	disable(successor, &d);
	expect_gone(successor, retained.fb);
	CHECK(close(successor) == 0);
	puts("PASS: CastKMS master handoff, unaccepted updates, retained files, RMFB, disable");
	return 0;
}
