// SPDX-License-Identifier: GPL-2.0-only
#include <poll.h>
#include <string.h>
#include <sys/mman.h>
#include <drm_fourcc.h>

#include "fixture.h"

struct buffer create_buffer(int fd, uint32_t width, uint32_t height,
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

void destroy_buffer(int fd, struct buffer *b)
{
	struct drm_mode_destroy_dumb destroy = { .handle = b->dumb.handle };

	CHECK(drmModeRmFB(fd, b->fb) == 0);
	if (destroy.handle)
		CHECK(drmIoctl(fd, DRM_IOCTL_MODE_DESTROY_DUMB, &destroy) == 0);
}

void property(int fd, drmModeAtomicReq *req, uint32_t id, uint32_t type,
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

void flip(int fd, uint32_t plane, uint32_t fb)
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
