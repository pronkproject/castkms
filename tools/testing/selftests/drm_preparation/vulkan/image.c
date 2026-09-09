// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "image.h"

static int image_supported(struct gpu_device *device, uint32_t width, uint32_t height,
			   uint64_t modifier)
{
	VkDrmFormatModifierPropertiesListEXT modifiers = {
		.sType = VK_STRUCTURE_TYPE_DRM_FORMAT_MODIFIER_PROPERTIES_LIST_EXT,
	};
	VkFormatProperties2 format = { .sType = VK_STRUCTURE_TYPE_FORMAT_PROPERTIES_2, .pNext = &modifiers };
	VkPhysicalDeviceImageDrmFormatModifierInfoEXT layout = {
		.sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_IMAGE_DRM_FORMAT_MODIFIER_INFO_EXT,
		.drmFormatModifier = modifier,
		.sharingMode = VK_SHARING_MODE_EXCLUSIVE,
	};
	VkPhysicalDeviceExternalImageFormatInfo external = {
		.sType = VK_STRUCTURE_TYPE_PHYSICAL_DEVICE_EXTERNAL_IMAGE_FORMAT_INFO,
		.pNext = &layout,
		.handleType = VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT,
	};
	VkPhysicalDeviceImageFormatInfo2 info = {
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
	VkFormatFeatureFlags blit = VK_FORMAT_FEATURE_BLIT_SRC_BIT | VK_FORMAT_FEATURE_BLIT_DST_BIT;
	VkExternalMemoryFeatureFlags sharing = VK_EXTERNAL_MEMORY_FEATURE_IMPORTABLE_BIT |
					      VK_EXTERNAL_MEMORY_FEATURE_EXPORTABLE_BIT;
	uint32_t i;
	int found = 0;

	vkGetPhysicalDeviceFormatProperties2(device->context->physical, info.format, &format);
	if (!modifiers.drmFormatModifierCount)
		return 0;
	modifiers.pDrmFormatModifierProperties = calloc(modifiers.drmFormatModifierCount,
						       sizeof(*modifiers.pDrmFormatModifierProperties));
	if (!modifiers.pDrmFormatModifierProperties)
		return 0;
	vkGetPhysicalDeviceFormatProperties2(device->context->physical, info.format, &format);
	for (i = 0; i < modifiers.drmFormatModifierCount; i++) {
		VkDrmFormatModifierPropertiesEXT *entry = &modifiers.pDrmFormatModifierProperties[i];

		if (entry->drmFormatModifier == modifier && entry->drmFormatModifierPlaneCount == 1 &&
		    (entry->drmFormatModifierTilingFeatures & blit) == blit)
			found = 1;
	}
	free(modifiers.pDrmFormatModifierProperties);
	return found && vkGetPhysicalDeviceImageFormatProperties2(device->context->physical, &info,
								 &properties) == VK_SUCCESS &&
		(memory.externalMemoryProperties.externalMemoryFeatures & sharing) == sharing &&
		width && height && width <= properties.imageFormatProperties.maxExtent.width &&
		height <= properties.imageFormatProperties.maxExtent.height;
}

static VkImageCreateInfo image_info(uint32_t width, uint32_t height, const void *external)
{
	return (VkImageCreateInfo) {
		.sType = VK_STRUCTURE_TYPE_IMAGE_CREATE_INFO,
		.pNext = external,
		.imageType = VK_IMAGE_TYPE_2D,
		.format = VK_FORMAT_B8G8R8A8_UNORM,
		.extent = { width, height, 1 },
		.mipLevels = 1,
		.arrayLayers = 1,
		.samples = VK_SAMPLE_COUNT_1_BIT,
		.tiling = VK_IMAGE_TILING_DRM_FORMAT_MODIFIER_EXT,
		.usage = VK_IMAGE_USAGE_TRANSFER_SRC_BIT | VK_IMAGE_USAGE_TRANSFER_DST_BIT,
		.sharingMode = VK_SHARING_MODE_EXCLUSIVE,
		.initialLayout = VK_IMAGE_LAYOUT_UNDEFINED,
	};
}

int gpu_image_create(struct gpu_device *device, struct gpu_image *image,
		     uint32_t width, uint32_t height, uint64_t modifier)
{
	VkImageDrmFormatModifierListCreateInfoEXT layout = {
		.sType = VK_STRUCTURE_TYPE_IMAGE_DRM_FORMAT_MODIFIER_LIST_CREATE_INFO_EXT,
		.drmFormatModifierCount = 1,
		.pDrmFormatModifiers = &modifier,
	};
	VkExternalMemoryImageCreateInfo external = {
		.sType = VK_STRUCTURE_TYPE_EXTERNAL_MEMORY_IMAGE_CREATE_INFO,
		.pNext = &layout,
		.handleTypes = VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT,
	};
	VkImageCreateInfo info = image_info(width, height, &external);
	VkMemoryDedicatedAllocateInfo dedicated = {
		.sType = VK_STRUCTURE_TYPE_MEMORY_DEDICATED_ALLOCATE_INFO,
	};
	VkExportMemoryAllocateInfo export = {
		.sType = VK_STRUCTURE_TYPE_EXPORT_MEMORY_ALLOCATE_INFO,
		.pNext = &dedicated,
		.handleTypes = VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT,
	};
	VkMemoryAllocateInfo allocation = {
		.sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO,
		.pNext = &export,
	};
	VkMemoryRequirements requirements;

	memset(image, 0, sizeof(*image));
	if (!image_supported(device, width, height, modifier))
		goto fail;
	if (vkCreateImage(device->handle, &info, NULL, &image->handle) != VK_SUCCESS)
		goto fail;
	dedicated.image = image->handle;
	vkGetImageMemoryRequirements(device->handle, image->handle, &requirements);
	allocation.allocationSize = requirements.size;
	if (gpu_memory_type(device, requirements.memoryTypeBits, VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT,
			    &allocation.memoryTypeIndex))
		goto fail;
	if (vkAllocateMemory(device->handle, &allocation, NULL, &image->memory) != VK_SUCCESS)
		goto fail;
	if (vkBindImageMemory(device->handle, image->handle, image->memory, 0) != VK_SUCCESS)
		goto fail;
	image->allocation_size = allocation.allocationSize;
	image->width = width;
	image->height = height;
	return 0;
fail:
	fprintf(stderr, "DMA-BUF image allocation failed\n");
	gpu_image_destroy(device, image);
	return -1;
}

int gpu_image_export(struct gpu_device *device, const struct gpu_image *image,
		     struct gpu_image_description *description, int *fd)
{
	VkImageDrmFormatModifierPropertiesEXT modifier = {
		.sType = VK_STRUCTURE_TYPE_IMAGE_DRM_FORMAT_MODIFIER_PROPERTIES_EXT,
	};
	VkImageSubresource subresource = { .aspectMask = VK_IMAGE_ASPECT_MEMORY_PLANE_0_BIT_EXT };
	VkMemoryGetFdInfoKHR info = {
		.sType = VK_STRUCTURE_TYPE_MEMORY_GET_FD_INFO_KHR,
		.memory = image->memory,
		.handleType = VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT,
	};

	memset(description, 0, sizeof(*description));
	*fd = -1;
	if (device->image_modifier(device->handle, image->handle, &modifier) != VK_SUCCESS)
		return -1;
	description->width = image->width;
	description->height = image->height;
	description->modifier = modifier.drmFormatModifier;
	description->allocation_size = image->allocation_size;
	vkGetImageSubresourceLayout(device->handle, image->handle, &subresource, &description->plane);
	/* Explicit modifier imports take pitch/offset, not the queried byte extent. */
	description->plane.size = 0;
	description->plane.arrayPitch = 0;
	description->plane.depthPitch = 0;
	return device->memory_fd(device->handle, &info, fd) == VK_SUCCESS ? 0 : -1;
}

int gpu_image_import(struct gpu_device *device, struct gpu_image *image,
		     const struct gpu_image_description *description, int *fd)
{
	VkImageDrmFormatModifierExplicitCreateInfoEXT layout = {
		.sType = VK_STRUCTURE_TYPE_IMAGE_DRM_FORMAT_MODIFIER_EXPLICIT_CREATE_INFO_EXT,
		.drmFormatModifier = description->modifier,
		.drmFormatModifierPlaneCount = 1,
		.pPlaneLayouts = &description->plane,
	};
	VkExternalMemoryImageCreateInfo external = {
		.sType = VK_STRUCTURE_TYPE_EXTERNAL_MEMORY_IMAGE_CREATE_INFO,
		.pNext = &layout,
		.handleTypes = VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT,
	};
	VkImageCreateInfo info = image_info(description->width, description->height, &external);
	VkMemoryDedicatedAllocateInfo dedicated = {
		.sType = VK_STRUCTURE_TYPE_MEMORY_DEDICATED_ALLOCATE_INFO,
	};
	VkImportMemoryFdInfoKHR import = {
		.sType = VK_STRUCTURE_TYPE_IMPORT_MEMORY_FD_INFO_KHR,
		.pNext = &dedicated,
		.handleType = VK_EXTERNAL_MEMORY_HANDLE_TYPE_DMA_BUF_BIT_EXT,
		.fd = *fd,
	};
	VkMemoryFdPropertiesKHR properties = { .sType = VK_STRUCTURE_TYPE_MEMORY_FD_PROPERTIES_KHR };
	VkMemoryAllocateInfo allocation = {
		.sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO,
		.pNext = &import,
		.allocationSize = description->allocation_size,
	};
	VkMemoryRequirements requirements;

	memset(image, 0, sizeof(*image));
	if (vkCreateImage(device->handle, &info, NULL, &image->handle) != VK_SUCCESS)
		goto fail;
	dedicated.image = image->handle;
	vkGetImageMemoryRequirements(device->handle, image->handle, &requirements);
	if (requirements.size > allocation.allocationSize ||
	    device->memory_fd_properties(device->handle, import.handleType, *fd, &properties) != VK_SUCCESS)
		goto fail;
	if (gpu_memory_type(device, properties.memoryTypeBits & requirements.memoryTypeBits,
			    VK_MEMORY_PROPERTY_DEVICE_LOCAL_BIT, &allocation.memoryTypeIndex))
		goto fail;
	if (vkAllocateMemory(device->handle, &allocation, NULL, &image->memory) != VK_SUCCESS)
		goto fail;
	*fd = -1;
	if (vkBindImageMemory(device->handle, image->handle, image->memory, 0) != VK_SUCCESS)
		goto fail;
	image->allocation_size = allocation.allocationSize;
	image->width = description->width;
	image->height = description->height;
	return 0;
fail:
	fprintf(stderr, "DMA-BUF image import failed\n");
	gpu_image_destroy(device, image);
	return -1;
}

void gpu_image_destroy(struct gpu_device *device, struct gpu_image *image)
{
	if (!device->handle)
		return;
	vkDestroyImage(device->handle, image->handle, NULL);
	vkFreeMemory(device->handle, image->memory, NULL);
	memset(image, 0, sizeof(*image));
}
