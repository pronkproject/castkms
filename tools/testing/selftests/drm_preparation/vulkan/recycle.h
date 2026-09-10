/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef DRM_PREPARATION_VULKAN_RECYCLE_H
#define DRM_PREPARATION_VULKAN_RECYCLE_H

#include "image.h"

/* A persistent allocation and its cached import, exchanged between two devices. */
struct gpu_recycle_image {
	struct gpu_device *devices[2];
	struct gpu_image images[2];
	unsigned int owner;
	int initialized;
	int external;
	int completion_fd;
};

int gpu_recycle_create(struct gpu_recycle_image *image, struct gpu_device *writer,
		       struct gpu_device *reader, uint64_t modifier);
void gpu_recycle_destroy(struct gpu_recycle_image *image);

/* Operations finish their native submissions before returning. Any error is terminal. */
int gpu_recycle_source(struct gpu_recycle_image *staging, unsigned int color, uint64_t modifier);
int gpu_recycle_copy(struct gpu_recycle_image *staging, struct gpu_recycle_image *output);
int gpu_recycle_check(struct gpu_recycle_image *output, unsigned int color);
int gpu_recycle_return(struct gpu_recycle_image *output);

#endif
