/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef DRM_PREPARATION_VULKAN_CONTEXT_H
#define DRM_PREPARATION_VULKAN_CONTEXT_H

#include <vulkan/vulkan.h>
#include <stdatomic.h>

struct gpu_context {
	VkInstance instance;
	VkPhysicalDevice physical;
	VkPhysicalDeviceProperties properties;
	uint32_t queue_family;
	VkDebugUtilsMessengerEXT debug;
	atomic_uint validation_errors;
};

/* The caller selects an actual render node; no software or different-device fallback. */
int gpu_context_open(struct gpu_context *context, const char *render_node, int validation);
void gpu_context_close(struct gpu_context *context);
/* One means supported, zero means absent, and a negative result is a query failure. */
int gpu_has_extension(VkPhysicalDevice physical, const char *name);

#endif
