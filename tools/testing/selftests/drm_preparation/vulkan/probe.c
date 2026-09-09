// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <inttypes.h>
#include <errno.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "context.h"

static int probe(struct gpu_context *context)
{
	const char *required[] = {
		VK_KHR_EXTERNAL_MEMORY_FD_EXTENSION_NAME,
		VK_EXT_EXTERNAL_MEMORY_DMA_BUF_EXTENSION_NAME,
		VK_EXT_IMAGE_DRM_FORMAT_MODIFIER_EXTENSION_NAME,
		VK_KHR_EXTERNAL_SEMAPHORE_FD_EXTENSION_NAME,
	};
	VkDrmFormatModifierPropertiesListEXT modifiers = {
		.sType = VK_STRUCTURE_TYPE_DRM_FORMAT_MODIFIER_PROPERTIES_LIST_EXT,
	};
	VkFormatProperties2 format = {
		.sType = VK_STRUCTURE_TYPE_FORMAT_PROPERTIES_2,
		.pNext = &modifiers,
	};
	VkPhysicalDeviceExternalSemaphoreInfo semaphore_info = {
		.sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_EXTERNAL_SEMAPHORE_INFO,
		.handleType = VK_EXTERNAL_SEMAPHORE_HANDLE_TYPE_SYNC_FD_BIT,
	};
	VkExternalSemaphoreProperties semaphore = {
		.sType = VK_STRUCTURE_TYPE_EXTERNAL_SEMAPHORE_PROPERTIES,
	};
	unsigned int i, usable = 0;

	for (i = 0; i < sizeof(required) / sizeof(required[0]); i++) {
		int supported = gpu_has_extension(context->physical, required[i]);

		if (supported <= 0) {
			printf("extension=%s support=%d\n", required[i], supported);
			return supported ? 1 : 4;
		}
	}
	vkGetPhysicalDeviceExternalSemaphoreProperties(context->physical, &semaphore_info, &semaphore);
	printf("sync_fd_features=0x%x compatible_handles=0x%x\n",
	       semaphore.externalSemaphoreFeatures, semaphore.compatibleHandleTypes);
	if ((semaphore.externalSemaphoreFeatures &
	     (VK_EXTERNAL_SEMAPHORE_FEATURE_EXPORTABLE_BIT | VK_EXTERNAL_SEMAPHORE_FEATURE_IMPORTABLE_BIT)) !=
	    (VK_EXTERNAL_SEMAPHORE_FEATURE_EXPORTABLE_BIT | VK_EXTERNAL_SEMAPHORE_FEATURE_IMPORTABLE_BIT))
		return 4;
	vkGetPhysicalDeviceFormatProperties2(context->physical, VK_FORMAT_B8G8R8A8_UNORM, &format);
	modifiers.pDrmFormatModifierProperties = calloc(modifiers.drmFormatModifierCount,
						       sizeof(*modifiers.pDrmFormatModifierProperties));
	if (!modifiers.pDrmFormatModifierProperties)
		return 1;
	vkGetPhysicalDeviceFormatProperties2(context->physical, VK_FORMAT_B8G8R8A8_UNORM, &format);
	for (i = 0; i < modifiers.drmFormatModifierCount; i++) {
		VkDrmFormatModifierPropertiesEXT *modifier = &modifiers.pDrmFormatModifierProperties[i];
		VkPhysicalDeviceImageDrmFormatModifierInfoEXT layout = {
			.sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_IMAGE_DRM_FORMAT_MODIFIER_INFO_EXT,
			.drmFormatModifier = modifier->drmFormatModifier,
			.sharingMode = VK_SHARING_MODE_EXCLUSIVE,
		};
		VkPhysicalDeviceExternalImageFormatInfo external = {
			.sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_EXTERNAL_IMAGE_FORMAT_INFO,
			.pNext = &layout,
			.handleType = VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT,
		};
		VkPhysicalDeviceImageFormatInfo2 image = {
			.sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_IMAGE_FORMAT_INFO_2,
			.pNext = &external,
			.format = VK_FORMAT_B8G8R8A8_UNORM,
			.type = VK_IMAGE_TYPE_2D,
			.tiling = VK_IMAGE_TILING_DRM_FORMAT_MODIFIER_EXT,
			.usage = VK_IMAGE_USAGE_TRANSFER_SRC_BIT | VK_IMAGE_USAGE_TRANSFER_DST_BIT,
		};
		VkExternalImageFormatProperties memory = {
			.sType = VK_STRUCTURE_TYPE_EXTERNAL_IMAGE_FORMAT_PROPERTIES,
		};
		VkImageFormatProperties2 properties = {
			.sType = VK_STRUCTURE_TYPE_IMAGE_FORMAT_PROPERTIES_2,
			.pNext = &memory,
		};
		VkResult result = vkGetPhysicalDeviceImageFormatProperties2(context->physical, &image,
									    &properties);
		VkFormatFeatureFlags blit = VK_FORMAT_FEATURE_BLIT_SRC_BIT | VK_FORMAT_FEATURE_BLIT_DST_BIT;
		VkExternalMemoryFeatureFlags sharing = VK_EXTERNAL_MEMORY_FEATURE_EXPORTABLE_BIT |
						      VK_EXTERNAL_MEMORY_FEATURE_IMPORTABLE_BIT;
		int supported = result == VK_SUCCESS && modifier->drmFormatModifierPlaneCount == 1 &&
			(modifier->drmFormatModifierTilingFeatures & blit) == blit &&
			(memory.externalMemoryProperties.externalMemoryFeatures & sharing) == sharing;

		if (result != VK_SUCCESS && result != VK_ERROR_FORMAT_NOT_SUPPORTED) {
			fprintf(stderr, "External image format query failed: %d\n", result);
			free(modifiers.pDrmFormatModifierProperties);
			return 1;
		}
		printf("format=B8G8R8A8_UNORM modifier=0x%016" PRIx64
		       " planes=%u features=0x%x external=0x%x query=%d single_plane_blit=%d\n",
		       modifier->drmFormatModifier, modifier->drmFormatModifierPlaneCount,
		       modifier->drmFormatModifierTilingFeatures,
		       memory.externalMemoryProperties.externalMemoryFeatures, result, supported);
		usable += supported;
	}
	free(modifiers.pDrmFormatModifierProperties);
	return usable ? 0 : 4;
}

int main(int argc, char **argv)
{
	struct gpu_context context;
	int result;

	if ((argc != 2 && argc != 3) || (argc == 3 && strcmp(argv[2], "--validation"))) {
		fprintf(stderr, "Usage: %s RENDER_NODE [--validation]\n", argv[0]);
		return 1;
	}
	result = gpu_context_open(&context, argv[1], argc == 3);
	if (result)
		return result == -ENODEV ? 4 : 1;
	result = probe(&context);
	gpu_context_close(&context);
	if (atomic_load(&context.validation_errors))
		return 1;
	return result;
}
