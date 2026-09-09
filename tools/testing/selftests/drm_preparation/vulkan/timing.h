/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef DRM_PREPARATION_VULKAN_TIMING_H
#define DRM_PREPARATION_VULKAN_TIMING_H

#include "device.h"
#include <time.h>

struct gpu_timing {
	VkQueryPool pool;
	uint32_t valid_bits;
	double period_ns;
	struct timespec created;
};

/* Optional instrumentation: an empty owner records no commands and reports no sample. */
int gpu_timing_create(struct gpu_device *device, struct gpu_timing *timing);
void gpu_timing_begin(VkCommandBuffer commands, const struct gpu_timing *timing);
void gpu_timing_end(VkCommandBuffer commands, const struct gpu_timing *timing);
/* Call only after the containing submission has completed. No cross-device subtraction. */
int gpu_timing_report(struct gpu_device *device, const struct gpu_timing *timing,
		      const char *stage, unsigned int frame);
/* Finish submitted commands before destroying their query storage. */
void gpu_timing_destroy(struct gpu_device *device, struct gpu_timing *timing);

#endif
