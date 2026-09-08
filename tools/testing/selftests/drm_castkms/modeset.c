// SPDX-License-Identifier: GPL-2.0-only
/* Run only against an explicitly selected, disposable Rust CastKMS device. */
#include <errno.h>
#include <fcntl.h>
#include <poll.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>
#include <xf86drm.h>
#include <xf86drmMode.h>
#include <drm_fourcc.h>

#define CHECK(expr) do { \
	if (!(expr)) { \
		fprintf(stderr, "%s:%d: %s (errno %d)\n", \
			__FILE__, __LINE__, #expr, errno); \
		exit(1); \
	} \
} while (0)

struct buffer {
	struct drm_mode_create_dumb dumb;
	uint32_t fb;
};

static struct buffer create_buffer(int fd, uint32_t width, uint32_t height,
				   unsigned char pixel)
{
	struct buffer b = { .dumb = { .width = width, .height = height, .bpp = 32 } };
	struct drm_mode_map_dumb map = {0};
	uint32_t handles[4] = {0}, pitches[4] = {0}, offsets[4] = {0};
	void *data;

	CHECK(drmIoctl(fd, DRM_IOCTL_MODE_CREATE_DUMB, &b.dumb) == 0);
	map.handle = b.dumb.handle;
	CHECK(drmIoctl(fd, DRM_IOCTL_MODE_MAP_DUMB, &map) == 0);
	data = mmap(NULL, b.dumb.size, PROT_READ | PROT_WRITE, MAP_SHARED, fd, map.offset);
	CHECK(data != MAP_FAILED);
	memset(data, pixel, b.dumb.size);
	CHECK(munmap(data, b.dumb.size) == 0);
	handles[0] = b.dumb.handle;
	pitches[0] = b.dumb.pitch;
	CHECK(drmModeAddFB2(fd, width, height, DRM_FORMAT_XRGB8888,
			    handles, pitches, offsets, &b.fb, 0) == 0);
	return b;
}

static void destroy_buffer(int fd, struct buffer *b)
{
	struct drm_mode_destroy_dumb destroy = { .handle = b->dumb.handle };

	CHECK(drmModeRmFB(fd, b->fb) == 0);
	CHECK(drmIoctl(fd, DRM_IOCTL_MODE_DESTROY_DUMB, &destroy) == 0);
}

static void property(int fd, drmModeAtomicReq *req, uint32_t id, uint32_t type,
		     const char *name, uint64_t value)
{
	drmModeObjectProperties *props = drmModeObjectGetProperties(fd, id, type);
	uint32_t found = 0;

	CHECK(props);
	for (uint32_t i = 0; i < props->count_props; i++) {
		drmModePropertyRes *prop = drmModeGetProperty(fd, props->props[i]);

		CHECK(prop);
		if (!strcmp(prop->name, name))
			found = prop->prop_id;
		drmModeFreeProperty(prop);
	}
	drmModeFreeObjectProperties(props);
	CHECK(found);
	CHECK(drmModeAtomicAddProperty(req, id, found, value) >= 0);
}

static void page_flip(int fd, unsigned int sequence, unsigned int sec,
		      unsigned int usec, void *data)
{
	unsigned int *events = data;

	(void)fd;
	(void)sequence;
	(void)sec;
	(void)usec;
	(*events)++;
}

static void flip(int fd, uint32_t plane, uint32_t fb)
{
	drmModeAtomicReq *req = drmModeAtomicAlloc();
	drmEventContext context = { .version = 2, .page_flip_handler = page_flip };
	struct pollfd pollfd = { .fd = fd, .events = POLLIN };
	unsigned int events = 0;

	CHECK(req);
	property(fd, req, plane, DRM_MODE_OBJECT_PLANE, "FB_ID", fb);
	CHECK(drmModeAtomicCommit(fd, req, DRM_MODE_ATOMIC_NONBLOCK |
				 DRM_MODE_PAGE_FLIP_EVENT, &events) == 0);
	CHECK(poll(&pollfd, 1, 5000) == 1);
	CHECK(pollfd.revents == POLLIN);
	CHECK(drmHandleEvent(fd, &context) == 0);
	CHECK(events == 1);
	CHECK(poll(&pollfd, 1, 0) == 0);
	drmModeAtomicFree(req);
}

int main(int argc, char **argv)
{
	drmModeRes *resources;
	drmModeConnector *connector;
	drmModePlaneRes *planes;
	drmModeAtomicReq *req;
	drmModeCrtc *crtc;
	drmVersion *version;
	drmModeModeInfo *mode;
	struct buffer a, b;
	uint32_t crtc_id, connector_id, plane_id, mode_id;
	int fd;

	if (argc != 2) {
		fprintf(stderr, "SKIP: supply a disposable Rust CastKMS DRM node\n");
		return 4;
	}
	fd = open(argv[1], O_RDWR | O_CLOEXEC);
	CHECK(fd >= 0);
	version = drmGetVersion(fd);
	CHECK(version && !strcmp(version->name, "castkms"));
	CHECK(version->version_major == 0 && version->version_minor == 0);
	drmFreeVersion(version);
	CHECK(drmSetClientCap(fd, DRM_CLIENT_CAP_ATOMIC, 1) == 0);
	CHECK(drmSetMaster(fd) == 0);
	resources = drmModeGetResources(fd);
	CHECK(resources && resources->count_crtcs == 1 && resources->count_connectors == 1);
	crtc_id = resources->crtcs[0];
	connector_id = resources->connectors[0];
	connector = drmModeGetConnector(fd, connector_id);
	CHECK(connector && connector->connection == DRM_MODE_CONNECTED);
	CHECK(connector->count_modes > 0);
	mode = &connector->modes[0];
	planes = drmModeGetPlaneResources(fd);
	CHECK(planes && planes->count_planes == 1);
	plane_id = planes->planes[0];
	a = create_buffer(fd, mode->hdisplay, mode->vdisplay, 0x33);
	b = create_buffer(fd, mode->hdisplay, mode->vdisplay, 0x88);
	CHECK(drmModeCreatePropertyBlob(fd, mode, sizeof(*mode), &mode_id) == 0);
	req = drmModeAtomicAlloc();
	CHECK(req);
	property(fd, req, connector_id, DRM_MODE_OBJECT_CONNECTOR, "CRTC_ID", crtc_id);
	property(fd, req, crtc_id, DRM_MODE_OBJECT_CRTC, "ACTIVE", 1);
	property(fd, req, crtc_id, DRM_MODE_OBJECT_CRTC, "MODE_ID", mode_id);
	property(fd, req, plane_id, DRM_MODE_OBJECT_PLANE, "FB_ID", a.fb);
	property(fd, req, plane_id, DRM_MODE_OBJECT_PLANE, "CRTC_ID", crtc_id);
	property(fd, req, plane_id, DRM_MODE_OBJECT_PLANE, "SRC_X", 0);
	property(fd, req, plane_id, DRM_MODE_OBJECT_PLANE, "SRC_Y", 0);
	property(fd, req, plane_id, DRM_MODE_OBJECT_PLANE, "SRC_W", mode->hdisplay << 16);
	property(fd, req, plane_id, DRM_MODE_OBJECT_PLANE, "SRC_H", mode->vdisplay << 16);
	property(fd, req, plane_id, DRM_MODE_OBJECT_PLANE, "CRTC_X", 0);
	property(fd, req, plane_id, DRM_MODE_OBJECT_PLANE, "CRTC_Y", 0);
	property(fd, req, plane_id, DRM_MODE_OBJECT_PLANE, "CRTC_W", mode->hdisplay);
	property(fd, req, plane_id, DRM_MODE_OBJECT_PLANE, "CRTC_H", mode->vdisplay);
	CHECK(drmModeAtomicCommit(fd, req, DRM_MODE_ATOMIC_ALLOW_MODESET |
				 DRM_MODE_ATOMIC_TEST_ONLY, NULL) == 0);
	crtc = drmModeGetCrtc(fd, crtc_id);
	CHECK(crtc && !crtc->mode_valid);
	drmModeFreeCrtc(crtc);
	CHECK(drmModeAtomicCommit(fd, req, DRM_MODE_ATOMIC_ALLOW_MODESET, NULL) == 0);
	drmModeAtomicFree(req);
	crtc = drmModeGetCrtc(fd, crtc_id);
	CHECK(crtc && crtc->mode_valid && crtc->buffer_id == a.fb);
	drmModeFreeCrtc(crtc);
	for (unsigned int i = 0; i < 16; i++) {
		flip(fd, plane_id, b.fb);
		flip(fd, plane_id, b.fb);
		flip(fd, plane_id, a.fb);
	}
	req = drmModeAtomicAlloc();
	CHECK(req);
	property(fd, req, plane_id, DRM_MODE_OBJECT_PLANE, "CRTC_W", mode->hdisplay / 2);
	CHECK(drmModeAtomicCommit(fd, req, DRM_MODE_ATOMIC_TEST_ONLY, NULL) != 0);
	drmModeAtomicFree(req);
	req = drmModeAtomicAlloc();
	CHECK(req);
	property(fd, req, plane_id, DRM_MODE_OBJECT_PLANE, "FB_ID", 0);
	property(fd, req, plane_id, DRM_MODE_OBJECT_PLANE, "CRTC_ID", 0);
	property(fd, req, connector_id, DRM_MODE_OBJECT_CONNECTOR, "CRTC_ID", 0);
	property(fd, req, crtc_id, DRM_MODE_OBJECT_CRTC, "ACTIVE", 0);
	property(fd, req, crtc_id, DRM_MODE_OBJECT_CRTC, "MODE_ID", 0);
	CHECK(drmModeAtomicCommit(fd, req, DRM_MODE_ATOMIC_ALLOW_MODESET, NULL) == 0);
	drmModeAtomicFree(req);
	crtc = drmModeGetCrtc(fd, crtc_id);
	CHECK(crtc && !crtc->mode_valid);
	drmModeFreeCrtc(crtc);
	destroy_buffer(fd, &b);
	destroy_buffer(fd, &a);
	CHECK(drmModeDestroyPropertyBlob(fd, mode_id) == 0);
	drmModeFreePlaneResources(planes);
	drmModeFreeConnector(connector);
	drmModeFreeResources(resources);
	CHECK(close(fd) == 0);
	puts("PASS: CastKMS allocation, modeset, 48 flips, rejection, disable");
	return 0;
}
