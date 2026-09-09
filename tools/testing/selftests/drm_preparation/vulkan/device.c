// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <stdio.h>
#include <string.h>

#include "device.h"

static int open_device(struct gpu_device *device, struct gpu_context *context, int foreign)
{
	const char *extensions[] = {
		VK_KHR_EXTERNAL_MEMORY_FD_EXTENSION_NAME,
		VK_EXT_EXTERNAL_MEMORY_DMA_BUF_EXTENSION_NAME,
		VK_EXT_IMAGE_DRM_FORMAT_MODIFIER_EXTENSION_NAME,
		VK_KHR_EXTERNAL_SEMAPHORE_FD_EXTENSION_NAME,
		VK_EXT_QUEUE_FAMILY_FOREIGN_EXTENSION_NAME,
	};
	float priority = 1.0f;
	VkDeviceQueueCreateInfo queue = {
		.sType = VK_STRUCTURE_TYPE_DEVICE_QUEUE_CREATE_INFO,
		.queueFamilyIndex = context->queue_family,
		.queueCount = 1,
		.pQueuePriorities = &priority,
	};
	VkDeviceCreateInfo info = {
		.sType = VK_STRUCTURE_TYPE_DEVICE_CREATE_INFO,
		.queueCreateInfoCount = 1,
		.pQueueCreateInfos = &queue,
		.enabledExtensionCount = sizeof(extensions) / sizeof(extensions[0]) - !foreign,
		.ppEnabledExtensionNames = extensions,
	};
	VkCommandPoolCreateInfo pool = {
		.sType = VK_STRUCTURE_TYPE_COMMAND_POOL_CREATE_INFO,
		.queueFamilyIndex = context->queue_family,
	};
	VkResult result;

	memset(device, 0, sizeof(*device));
	device->context = context;
	result = vkCreateDevice(context->physical, &info, NULL, &device->handle);
	if (result != VK_SUCCESS)
		goto fail;
	vkGetDeviceQueue(device->handle, context->queue_family, 0, &device->queue);
	device->memory_fd = (PFN_vkGetMemoryFdKHR)vkGetDeviceProcAddr(device->handle, "vkGetMemoryFdKHR");
	device->memory_fd_properties = (PFN_vkGetMemoryFdPropertiesKHR)
		vkGetDeviceProcAddr(device->handle, "vkGetMemoryFdPropertiesKHR");
	device->image_modifier = (PFN_vkGetImageDrmFormatModifierPropertiesEXT)
		vkGetDeviceProcAddr(device->handle, "vkGetImageDrmFormatModifierPropertiesEXT");
	device->semaphore_fd = (PFN_vkGetSemaphoreFdKHR)
		vkGetDeviceProcAddr(device->handle, "vkGetSemaphoreFdKHR");
	device->import_semaphore_fd = (PFN_vkImportSemaphoreFdKHR)
		vkGetDeviceProcAddr(device->handle, "vkImportSemaphoreFdKHR");
	if (!device->memory_fd || !device->memory_fd_properties || !device->image_modifier ||
	    !device->semaphore_fd || !device->import_semaphore_fd) {
		result = VK_ERROR_EXTENSION_NOT_PRESENT;
		goto fail;
	}
	result = vkCreateCommandPool(device->handle, &pool, NULL, &device->pool);
	if (result == VK_SUCCESS)
		return 0;
fail:
	fprintf(stderr, "Vulkan device setup failed: %d\n", result);
	gpu_device_close(device);
	return -1;
}

int gpu_device_open(struct gpu_device *device, struct gpu_context *context)
{
	return open_device(device, context, 0);
}

int gpu_device_open_foreign(struct gpu_device *device, struct gpu_context *context)
{
	return open_device(device, context, 1);
}

int gpu_device_close(struct gpu_device *device)
{
	VkResult result = VK_SUCCESS;

	if (device->handle) {
		result = vkDeviceWaitIdle(device->handle);
		vkDestroyCommandPool(device->handle, device->pool, NULL);
		vkDestroyDevice(device->handle, NULL);
	}
	memset(device, 0, sizeof(*device));
	return result == VK_SUCCESS ? 0 : -1;
}

int gpu_memory_type(struct gpu_device *device, uint32_t bits,
		    VkMemoryPropertyFlags required, uint32_t *index)
{
	VkPhysicalDeviceMemoryProperties properties;
	uint32_t i;

	vkGetPhysicalDeviceMemoryProperties(device->context->physical, &properties);
	for (i = 0; i < properties.memoryTypeCount; i++) {
		if ((bits & (1u << i)) &&
		    (properties.memoryTypes[i].propertyFlags & required) == required) {
			*index = i;
			return 0;
		}
	}
	return -1;
}

int gpu_commands_begin(struct gpu_device *device, VkCommandBuffer *commands)
{
	VkCommandBufferAllocateInfo allocation = {
		.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_ALLOCATE_INFO,
		.commandPool = device->pool,
		.level = VK_COMMAND_BUFFER_LEVEL_PRIMARY,
		.commandBufferCount = 1,
	};
	VkCommandBufferBeginInfo begin = {
		.sType = VK_STRUCTURE_TYPE_COMMAND_BUFFER_BEGIN_INFO,
		.flags = VK_COMMAND_BUFFER_USAGE_ONE_TIME_SUBMIT_BIT,
	};

	if (vkAllocateCommandBuffers(device->handle, &allocation, commands) != VK_SUCCESS)
		return -1;
	return vkBeginCommandBuffer(*commands, &begin) == VK_SUCCESS ? 0 : -1;
}

int gpu_semaphore_create(struct gpu_device *device, VkSemaphore *semaphore)
{
	VkPhysicalDeviceExternalSemaphoreInfo query = {
		.sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_EXTERNAL_SEMAPHORE_INFO,
		.handleType = VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_SYNC_FD_BIT,
	};
	VkExternalSemaphoreProperties properties = {
		.sType = VK_STRUCTURE_TYPE_EXTERNAL_SEMAPHORE_PROPERTIES,
	};
	VkExternalSemaphoreFeatureFlags required = VK_EXTERNAL_SEMAPHORE_FEATURE_IMPORTABLE_BIT |
						   VK_EXTERNAL_SEMAPHORE_FEATURE_EXPORTABLE_BIT;
	VkExportSemaphoreCreateInfo external = {
		.sType = VK_STRUCTURE_TYPE_EXPORT_SEMAPHORE_CREATE_INFO,
		.handleTypes = VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_SYNC_FD_BIT,
	};
	VkSemaphoreCreateInfo info = {
		.sType = VK_STRUCTURE_TYPE_SEMAPHORE_CREATE_INFO,
		.pNext = &external,
	};

	vkGetPhysicalDeviceExternalSemaphoreProperties(device->context->physical, &query, &properties);
	if ((properties.externalSemaphoreFeatures & required) != required)
		return -1;
	return vkCreateSemaphore(device->handle, &info, NULL, semaphore) == VK_SUCCESS ? 0 : -1;
}

int gpu_semaphore_import(struct gpu_device *device, VkSemaphore semaphore, int fd)
{
	VkImportSemaphoreFdInfoKHR info = {
		.sType = VK_STRUCTURE_TYPE_IMPORT_SEMAPHORE_FD_INFO_KHR,
		.semaphore = semaphore,
		.flags = VK_SEMAPHORE_IMPORT_TEMPORARY_BIT,
		.handleType = VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_SYNC_FD_BIT,
		.fd = fd,
	};

	return device->import_semaphore_fd(device->handle, &info) == VK_SUCCESS ? 0 : -1;
}

int gpu_semaphore_export(struct gpu_device *device, VkSemaphore semaphore, int *fd)
{
	VkSemaphoreGetFdInfoKHR info = {
		.sType = VK_STRUCTURE_TYPE_SEMAPHORE_GET_FD_INFO_KHR,
		.semaphore = semaphore,
		.handleType = VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_SYNC_FD_BIT,
	};

	return device->semaphore_fd(device->handle, &info, fd) == VK_SUCCESS ? 0 : -1;
}
