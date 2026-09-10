// SPDX-License-Identifier: GPL-2.0-only
/* Private heap storage for testing foreign PRIME imports without mapping pixels. */
#include <fcntl.h>
#include <linux/dma-heap.h>
#include <sys/ioctl.h>
#include <unistd.h>
#include <drm_fourcc.h>

#include "fixture.h"

struct buffer import_buffer(int fd, const char *heap, uint32_t width,
			    uint32_t height)
{
	struct buffer buffer = {0};
	struct dma_heap_allocation_data allocation = {
		.len = (uint64_t)width * height * 4,
		.fd_flags = O_RDWR | O_CLOEXEC,
	};
	uint32_t handles[4] = {0}, pitches[4] = {width * 4}, offsets[4] = {0};
	uint64_t modifiers[4] = {DRM_FORMAT_MOD_LINEAR};
	int heap_fd = open(heap, O_RDWR | O_CLOEXEC);

	CHECK(heap_fd >= 0);
	CHECK(ioctl(heap_fd, DMA_HEAP_IOCTL_ALLOC, &allocation) == 0);
	CHECK(close(heap_fd) == 0);
	CHECK(drmPrimeFDToHandle(fd, allocation.fd, &handles[0]) == 0);
	CHECK(drmModeAddFB2WithModifiers(fd, width, height, DRM_FORMAT_XRGB8888,
					 handles, pitches, offsets, modifiers,
					 &buffer.fb, DRM_MODE_FB_MODIFIERS) == 0);
	/* Only the framebuffer retains imported storage when atomic submission starts. */
	CHECK(drmCloseBufferHandle(fd, handles[0]) == 0);
	CHECK(close(allocation.fd) == 0);
	return buffer;
}
