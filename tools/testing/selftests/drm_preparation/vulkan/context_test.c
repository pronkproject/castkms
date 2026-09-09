// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <errno.h>
#include <stdint.h>
#include <stdio.h>

#include "context.h"

static const struct test_case {
	const char *name;
	VkResult instance_result;
	VkResult count_result;
	VkResult list_result;
	uint32_t device_count;
	int expected;
	unsigned int expected_enumerations;
} cases[] = {
	{
		.name = "instance allocation failure",
		.instance_result = VK_ERROR_OUT_OF_HOST_MEMORY,
		.expected = -EIO,
	},
	{
		.name = "incompatible driver",
		.instance_result = VK_ERROR_INCOMPATIBLE_DRIVER,
		.expected = -ENODEV,
	},
	{
		.name = "device count allocation failure",
		.count_result = VK_ERROR_OUT_OF_HOST_MEMORY,
		.expected = -EIO,
		.expected_enumerations = 1,
	},
	{
		.name = "device count initialization failure",
		.count_result = VK_ERROR_INITIALIZATION_FAILED,
		.expected = -EIO,
		.expected_enumerations = 1,
	},
	{
		.name = "empty device list",
		.expected = -ENODEV,
		.expected_enumerations = 1,
	},
	{
		.name = "device list allocation failure",
		.list_result = VK_ERROR_OUT_OF_HOST_MEMORY,
		.device_count = 1,
		.expected = -EIO,
		.expected_enumerations = 2,
	},
	{
		.name = "incomplete device list",
		.list_result = VK_INCOMPLETE,
		.device_count = 1,
		.expected = -EIO,
		.expected_enumerations = 2,
	},
};

static const struct test_case *current;
static unsigned int enumerations, destructions;

VKAPI_ATTR VkResult VKAPI_CALL __wrap_vkCreateInstance(const VkInstanceCreateInfo *create,
						       const VkAllocationCallbacks *allocator,
						       VkInstance *instance)
{
	(void)create;
	(void)allocator;
	*instance = current->instance_result == VK_SUCCESS ? (VkInstance)(uintptr_t)1 : VK_NULL_HANDLE;
	return current->instance_result;
}

VKAPI_ATTR VkResult VKAPI_CALL __wrap_vkEnumeratePhysicalDevices(VkInstance instance,
							uint32_t *count, VkPhysicalDevice *devices)
{
	(void)instance;
	++enumerations;
	*count = current->device_count;
	return devices ? current->list_result : current->count_result;
}

VKAPI_ATTR void VKAPI_CALL __wrap_vkDestroyInstance(VkInstance instance,
						    const VkAllocationCallbacks *allocator)
{
	(void)instance;
	(void)allocator;
	++destructions;
}

int main(void)
{
	unsigned int i;
	int failures = 0;

	puts("TAP version 13");
	printf("1..%zu\n", sizeof(cases) / sizeof(cases[0]));
	for (i = 0; i < sizeof(cases) / sizeof(cases[0]); i++) {
		struct gpu_context context;
		int result, passed;

		current = &cases[i];
		enumerations = destructions = 0;
		/* The wrapped calls never select a physical device or submit GPU work. */
		result = gpu_context_open(&context, "/dev/null", 0);
		gpu_context_close(&context);
		passed = result == current->expected && enumerations == current->expected_enumerations &&
			destructions == (current->instance_result == VK_SUCCESS) && !context.instance;
		printf("%s %u - %s\n", passed ? "ok" : "not ok", i + 1, current->name);
		if (!passed) {
			printf("# result=%d expected=%d enumerations=%u destructions=%u\n",
			       result, current->expected, enumerations, destructions);
			failures++;
		}
	}
	return failures ? 1 : 0;
}
