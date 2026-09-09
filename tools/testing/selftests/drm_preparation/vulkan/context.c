// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/sysmacros.h>

#include "context.h"

int gpu_has_extension(VkPhysicalDevice physical, const char *name)
{
	VkExtensionProperties *extensions;
	uint32_t count = 0, i;
	VkResult result;
	int found = 0;

	result = vkEnumerateDeviceExtensionProperties(physical, NULL, &count, NULL);
	if (result != VK_SUCCESS)
		return -EIO;
	if (!count)
		return 0;
	extensions = calloc(count, sizeof(*extensions));
	if (!extensions)
		return -ENOMEM;
	result = vkEnumerateDeviceExtensionProperties(physical, NULL, &count, extensions);
	if (result == VK_SUCCESS) {
		for (i = 0; i < count; i++)
			found |= !strcmp(extensions[i].extensionName, name);
	}
	free(extensions);
	return result == VK_SUCCESS ? found : -EIO;
}

static VKAPI_ATTR VkBool32 VKAPI_CALL validation_message(
	VkDebugUtilsMessageSeverityFlagBitsEXT severity, VkDebugUtilsMessageTypeFlagsEXT type,
	const VkDebugUtilsMessengerCallbackDataEXT *message, void *data)
{
	struct gpu_context *context = data;

	(void)type;
	if (severity & VK_DEBUG_UTILS_MESSAGE_SEVERITY_ERROR_BIT_EXT)
		atomic_fetch_add(&context->validation_errors, 1);
	fprintf(stderr, "Vulkan validation: %s\n", message->pMessage);
	return VK_FALSE;
}

int gpu_context_open(struct gpu_context *context, const char *render_node, int validation)
{
	const char *layer = "VK_LAYER_KHRONOS_validation";
	const char *debug_extension = VK_EXT_DEBUG_UTILS_EXTENSION_NAME;
	VkDebugUtilsMessengerCreateInfoEXT debug = {
		.sType = VK_STRUCTURE_TYPE_DEBUG_UTILS_MESSENGER_CREATE_INFO_EXT,
		.messageSeverity = VK_DEBUG_UTILS_MESSAGE_SEVERITY_WARNING_BIT_EXT |
				   VK_DEBUG_UTILS_MESSAGE_SEVERITY_ERROR_BIT_EXT,
		.messageType = VK_DEBUG_UTILS_MESSAGE_TYPE_GENERAL_BIT_EXT |
			       VK_DEBUG_UTILS_MESSAGE_TYPE_VALIDATION_BIT_EXT |
			       VK_DEBUG_UTILS_MESSAGE_TYPE_PERFORMANCE_BIT_EXT,
		.pfnUserCallback = validation_message,
		.pUserData = context,
	};
	VkApplicationInfo application = {
		.sType = VK_STRUCTURE_TYPE_APPLICATION_INFO,
		.pApplicationName = "drm-preparation-fixture",
		.apiVersion = VK_API_VERSION_1_3,
	};
	VkInstanceCreateInfo create = {
		.sType = VK_STRUCTURE_TYPE_INSTANCE_CREATE_INFO,
		.pNext = validation ? &debug : NULL,
		.pApplicationInfo = &application,
		.enabledLayerCount = validation ? 1 : 0,
		.ppEnabledLayerNames = &layer,
		.enabledExtensionCount = validation ? 1 : 0,
		.ppEnabledExtensionNames = &debug_extension,
	};
	VkPhysicalDevice *physical = NULL;
	VkQueueFamilyProperties *queues = NULL;
	struct stat node;
	uint32_t count = 0, i;
	VkResult result;
	int error = -ENODEV;

	memset(context, 0, sizeof(*context));
	atomic_init(&context->validation_errors, 0);
	if (stat(render_node, &node) || !S_ISCHR(node.st_mode)) {
		fprintf(stderr, "Not an accessible device node: %s\n", render_node);
		return -ENODEV;
	}
	result = vkCreateInstance(&create, NULL, &context->instance);
	if (result != VK_SUCCESS) {
		fprintf(stderr, "vkCreateInstance: %d\n", result);
		return result == VK_ERROR_INCOMPATIBLE_DRIVER ? -ENODEV : -EIO;
	}
	if (validation) {
		PFN_vkCreateDebugUtilsMessengerEXT create_debug = (PFN_vkCreateDebugUtilsMessengerEXT)
			vkGetInstanceProcAddr(context->instance, "vkCreateDebugUtilsMessengerEXT");

		if (!create_debug || create_debug(context->instance, &debug, NULL, &context->debug) != VK_SUCCESS) {
			error = -EIO;
			goto failed;
		}
	}
	result = vkEnumeratePhysicalDevices(context->instance, &count, NULL);
	if (result != VK_SUCCESS) {
		fprintf(stderr, "vkEnumeratePhysicalDevices (count): %d\n", result);
		error = -EIO;
		goto failed;
	}
	if (!count)
		goto failed;
	physical = calloc(count, sizeof(*physical));
	if (!physical) {
		error = -ENOMEM;
		goto failed;
	}
	result = vkEnumeratePhysicalDevices(context->instance, &count, physical);
	if (result != VK_SUCCESS) {
		fprintf(stderr, "vkEnumeratePhysicalDevices (list): %d\n", result);
		error = -EIO;
		goto failed;
	}
	for (i = 0; i < count; i++) {
		VkPhysicalDeviceDrmPropertiesEXT drm = {
			.sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_DRM_PROPERTIES_EXT,
		};
		VkPhysicalDeviceDriverProperties driver = {
			.sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_DRIVER_PROPERTIES,
			.pNext = &drm,
		};
		VkPhysicalDeviceProperties2 properties = {
			.sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_PROPERTIES_2,
			.pNext = &driver,
		};
		int supported;

		vkGetPhysicalDeviceProperties(physical[i], &properties.properties);
		if (properties.properties.apiVersion < VK_API_VERSION_1_3)
			continue;
		supported = gpu_has_extension(physical[i], VK_EXT_PHYSICAL_DEVICE_DRM_EXTENSION_NAME);
		if (supported < 0) {
			error = supported;
			goto failed;
		}
		if (!supported)
			continue;
		vkGetPhysicalDeviceProperties2(physical[i], &properties);
		if (!drm.hasRender || drm.renderMajor != major(node.st_rdev) ||
		    drm.renderMinor != minor(node.st_rdev))
			continue;
		context->physical = physical[i];
		context->properties = properties.properties;
		printf("render_node=%s device=%s vendor=%04x device_id=%04x\n",
		       render_node, context->properties.deviceName,
		       context->properties.vendorID, context->properties.deviceID);
		printf("driver=%s info=%s driver_id=%u api=%u.%u.%u\n",
		       driver.driverName, driver.driverInfo, driver.driverID,
		       VK_API_VERSION_MAJOR(properties.properties.apiVersion),
		       VK_API_VERSION_MINOR(properties.properties.apiVersion),
		       VK_API_VERSION_PATCH(properties.properties.apiVersion));
		break;
	}
	free(physical);
	physical = NULL;
	if (!context->physical) {
		fprintf(stderr, "No Vulkan 1.3 device matches render node %s\n", render_node);
		goto failed;
	}
	vkGetPhysicalDeviceQueueFamilyProperties(context->physical, &count, NULL);
	queues = calloc(count, sizeof(*queues));
	if (!queues) {
		error = -ENOMEM;
		goto failed;
	}
	vkGetPhysicalDeviceQueueFamilyProperties(context->physical, &count, queues);
	for (i = 0; i < count; i++) {
		if (queues[i].queueCount && (queues[i].queueFlags & VK_QUEUE_GRAPHICS_BIT)) {
			context->queue_family = i;
			free(queues);
			return 0;
		}
	}
failed:
	free(queues);
	free(physical);
	gpu_context_close(context);
	return error;
}

void gpu_context_close(struct gpu_context *context)
{
	if (context->debug) {
		PFN_vkDestroyDebugUtilsMessengerEXT destroy_debug = (PFN_vkDestroyDebugUtilsMessengerEXT)
			vkGetInstanceProcAddr(context->instance, "vkDestroyDebugUtilsMessengerEXT");

		destroy_debug(context->instance, context->debug, NULL);
		context->debug = VK_NULL_HANDLE;
	}
	if (context->instance)
		vkDestroyInstance(context->instance, NULL);
	context->instance = VK_NULL_HANDLE;
	context->physical = VK_NULL_HANDLE;
}
