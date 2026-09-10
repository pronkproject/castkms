/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef CASTKMS_SELFTEST_FIXTURE_H
#define CASTKMS_SELFTEST_FIXTURE_H

#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <xf86drm.h>
#include <xf86drmMode.h>

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

struct buffer create_buffer(int fd, uint32_t width, uint32_t height,
			    unsigned char pixel);
struct buffer import_buffer(int fd, const char *heap, uint32_t width,
			    uint32_t height);
void destroy_buffer(int fd, struct buffer *buffer);
void property(int fd, drmModeAtomicReq *req, uint32_t id, uint32_t type,
	      const char *name, uint64_t value);
void flip(int fd, uint32_t plane, uint32_t fb);

#endif
