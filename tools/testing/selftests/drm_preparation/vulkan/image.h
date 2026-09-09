/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef DRM_PREPARATION_VULKAN_IMAGE_H
#define DRM_PREPARATION_VULKAN_IMAGE_H

#include "device.h"

struct gpu_image {
	VkImage handle;
	VkDeviceMemory memory;
	VkDeviceSize allocation_size;
	uint32_t width;
	uint32_t height;
};

struct gpu_image_description {
	uint32_t width;
	uint32_t height;
	uint64_t modifier;
	VkSubresourceLayout plane;
	VkDeviceSize allocation_size;
};

/* One-plane B8G8R8A8_UNORM images, dedicated allocation, transfer source/destination. */
int gpu_image_create(struct gpu_device *device, struct gpu_image *image,
		     uint32_t width, uint32_t height, uint64_t modifier);
int gpu_image_export(struct gpu_device *device, const struct gpu_image *image,
		     struct gpu_image_description *description, int *fd);
/* Consumes fd on successful memory import, even if subsequent binding fails. */
int gpu_image_import(struct gpu_device *device, struct gpu_image *image,
		     const struct gpu_image_description *description, int *fd);
/* Caller must finish all submitted uses before destruction. */
void gpu_image_destroy(struct gpu_device *device, struct gpu_image *image);

#endif
