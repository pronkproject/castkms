// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <inttypes.h>
#include <math.h>
#include <stdio.h>
#include <stdlib.h>

#include "timing.h"

int gpu_timing_create(struct gpu_device *device, struct gpu_timing *timing)
{
	VkQueryPoolCreateInfo create = {
		.sType = VK_STRUCTURE_TYPE_QUERY_POOL_CREATE_INFO,
		.queryType = VK_QUERY_TYPE_TIMESTAMP,
		.queryCount = 2,
	};
	VkQueueFamilyProperties *queues;
	uint32_t count = 0;

	vkGetPhysicalDeviceQueueFamilyProperties(device->context->physical, &count, NULL);
	queues = calloc(count, sizeof(*queues));
	if (!queues)
		return -1;
	vkGetPhysicalDeviceQueueFamilyProperties(device->context->physical, &count, queues);
	if (device->context->queue_family >= count) {
		free(queues);
		return -1;
	}
	timing->valid_bits = queues[device->context->queue_family].timestampValidBits;
	free(queues);
	timing->period_ns = device->context->properties.limits.timestampPeriod;
	if (!timing->valid_bits || timing->valid_bits > 64 ||
	    !isfinite(timing->period_ns) || timing->period_ns <= 0) {
		fprintf(stderr, "Selected queue has no usable timestamp counter\n");
		return -1;
	}
	if (clock_gettime(CLOCK_MONOTONIC, &timing->created))
		return -1;
	return vkCreateQueryPool(device->handle, &create, NULL, &timing->pool) == VK_SUCCESS ? 0 : -1;
}

void gpu_timing_begin(VkCommandBuffer commands, const struct gpu_timing *timing)
{
	if (!timing->pool)
		return;
	vkCmdResetQueryPool(commands, timing->pool, 0, 2);
	vkCmdWriteTimestamp(commands, VK_PIPELINE_STAGE_TOP_OF_PIPE_BIT, timing->pool, 0);
}

void gpu_timing_end(VkCommandBuffer commands, const struct gpu_timing *timing)
{
	if (!timing->pool)
		return;
	vkCmdWriteTimestamp(commands, VK_PIPELINE_STAGE_BOTTOM_OF_PIPE_BIT, timing->pool, 1);
	/* Keep subsequent oracle copies from overlapping the recorded interval. */
	vkCmdPipelineBarrier(commands, VK_PIPELINE_STAGE_ALL_COMMANDS_BIT,
			     VK_PIPELINE_STAGE_ALL_COMMANDS_BIT, 0, 0, NULL, 0, NULL, 0, NULL);
}

int gpu_timing_report(struct gpu_device *device, const struct gpu_timing *timing,
		      const char *stage, unsigned int frame)
{
	uint64_t timestamps[2], mask, ticks;
	struct timespec now;
	double elapsed_ns;

	if (!timing->pool)
		return 0;
	if (vkGetQueryPoolResults(device->handle, timing->pool, 0, 2, sizeof(timestamps),
				 timestamps, sizeof(timestamps[0]), VK_QUERY_RESULT_64_BIT) != VK_SUCCESS)
		return -1;
	mask = UINT64_MAX >> (64 - timing->valid_bits);
	if (clock_gettime(CLOCK_MONOTONIC, &now))
		return -1;
	elapsed_ns = (now.tv_sec - timing->created.tv_sec) * 1000000000.0 +
		     now.tv_nsec - timing->created.tv_nsec;
	/* The whole owner lifetime bounds the interval; reject ambiguous counter wraps. */
	if (elapsed_ns < 0 || elapsed_ns >= ((double)mask + 1) * timing->period_ns) {
		fprintf(stderr, "Timestamp owner lifetime exceeds the counter's unambiguous interval\n");
		return -1;
	}
	ticks = (timestamps[1] - timestamps[0]) & mask;
	printf("gpu_interval stage=%s frame=%u ticks=%" PRIu64 " period_ns=%.9g valid_bits=%u ms=%.6f\n",
	       stage, frame, ticks, timing->period_ns, timing->valid_bits,
	       ticks * timing->period_ns / 1000000.0);
	return 0;
}

void gpu_timing_destroy(struct gpu_device *device, struct gpu_timing *timing)
{
	if (timing->pool)
		vkDestroyQueryPool(device->handle, timing->pool, NULL);
	timing->pool = VK_NULL_HANDLE;
}
