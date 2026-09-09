/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef DRM_PREPARATION_VULKAN_DEVICE_H
#define DRM_PREPARATION_VULKAN_DEVICE_H

#include "context.h"

struct gpu_device {
	struct gpu_context *context;
	VkDevice handle;
	VkQueue queue;
	VkCommandPool pool;
	PFN_vkGetMemoryFdKHR memory_fd;
	PFN_vkGetMemoryFdPropertiesKHR memory_fd_properties;
	PFN_vkGetImageDrmFormatModifierPropertiesEXT image_modifier;
	PFN_vkGetSemaphoreFdKHR semaphore_fd;
	PFN_vkImportSemaphoreFdKHR import_semaphore_fd;
};

int gpu_device_open(struct gpu_device *device, struct gpu_context *context);
/* Wait for submitted work before releasing the device, including on test failure. */
int gpu_device_close(struct gpu_device *device);
int gpu_memory_type(struct gpu_device *device, uint32_t bits,
		    VkMemoryPropertyFlags required, uint32_t *index);
int gpu_commands_begin(struct gpu_device *device, VkCommandBuffer *commands);
int gpu_semaphore_create(struct gpu_device *device, VkSemaphore *semaphore);
/* Import consumes fd only on success; -1 represents an already completed sync file. */
int gpu_semaphore_import(struct gpu_device *device, VkSemaphore semaphore, int fd);
/* Call only after the semaphore's signal operation has been submitted. */
int gpu_semaphore_export(struct gpu_device *device, VkSemaphore semaphore, int *fd);

#endif
