// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <errno.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

#include "image.h"

#define WIDTH 256
#define HEIGHT 256

static const VkImageSubresourceRange color_range = {
	.aspectMask = VK_IMAGE_ASPECT_COLOR_BIT,
	.levelCount = 1,
	.layerCount = 1,
};

static void whole_image_barrier(VkCommandBuffer commands, VkImageMemoryBarrier barrier)
{
	barrier.sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER;
	barrier.subresourceRange = color_range;

	vkCmdPipelineBarrier(commands, VK_PIPELINE_STAGE_ALL_COMMANDS_BIT,
			     VK_PIPELINE_STAGE_ALL_COMMANDS_BIT, 0, 0, NULL, 0, NULL, 1, &barrier);
}

struct readback {
	VkBuffer buffer;
	VkDeviceMemory memory;
};

static int readback_create(struct gpu_device *device, struct readback *readback)
{
	VkBufferCreateInfo info = {
		.sType = VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO,
		.size = WIDTH * HEIGHT * 4,
		.usage = VK_BUFFER_USAGE_TRANSFER_DST_BIT,
		.sharingMode = VK_SHARING_MODE_EXCLUSIVE,
	};
	VkMemoryAllocateInfo allocation = { .sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO };
	VkMemoryRequirements requirements;

	if (vkCreateBuffer(device->handle, &info, NULL, &readback->buffer) != VK_SUCCESS)
		return -1;
	vkGetBufferMemoryRequirements(device->handle, readback->buffer, &requirements);
	allocation.allocationSize = requirements.size;
	if (gpu_memory_type(device, requirements.memoryTypeBits,
			    VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT | VK_MEMORY_PROPERTY_HOST_COHERENT_BIT,
			    &allocation.memoryTypeIndex))
		return -1;
	if (vkAllocateMemory(device->handle, &allocation, NULL, &readback->memory) != VK_SUCCESS)
		return -1;
	return vkBindBufferMemory(device->handle, readback->buffer, readback->memory, 0) == VK_SUCCESS ? 0 : -1;
}

static int check_pixels(struct gpu_device *device, struct readback *readback)
{
	unsigned char *pixels;
	unsigned int i;
	int result = 0;

	if (vkMapMemory(device->handle, readback->memory, 0, VK_WHOLE_SIZE, 0, (void **)&pixels) != VK_SUCCESS)
		return -1;
	for (i = 0; i < WIDTH * HEIGHT; i++) {
		/* The producer clears opaque red; B8G8R8A8 stores blue first. */
		if (pixels[4 * i] || pixels[4 * i + 1] ||
		    pixels[4 * i + 2] != 255 || pixels[4 * i + 3] != 255) {
			fprintf(stderr, "Pixel %u mismatch: %u %u %u %u\n", i,
				pixels[4 * i], pixels[4 * i + 1], pixels[4 * i + 2], pixels[4 * i + 3]);
			result = -1;
			break;
		}
	}
	vkUnmapMemory(device->handle, readback->memory);
	return result;
}

static int handoff(struct gpu_context *context)
{
	struct gpu_device producer = { 0 }, consumer = { 0 };
	struct gpu_image source = { 0 }, imported = { 0 };
	struct gpu_image_description description;
	struct readback readback = { 0 };
	VkSemaphore produced = VK_NULL_HANDLE, acquired = VK_NULL_HANDLE;
	VkCommandBuffer commands;
	VkClearColorValue red = { .float32 = { 1.0f, 0.0f, 0.0f, 1.0f } };
	VkPipelineStageFlags stage = VK_PIPELINE_STAGE_ALL_COMMANDS_BIT;
	VkSubmitInfo submit = { .sType = VK_STRUCTURE_TYPE_SUBMIT_INFO };
	VkBufferImageCopy copy = {
		.imageSubresource = { .aspectMask = VK_IMAGE_ASPECT_COLOR_BIT, .layerCount = 1 },
		.imageExtent = { WIDTH, HEIGHT, 1 },
	};
	VkMemoryBarrier host = {
		.sType = VK_STRUCTURE_TYPE_MEMORY_BARRIER,
		.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT,
		.dstAccessMask = VK_ACCESS_HOST_READ_BIT,
	};
	int memory_fd = -1, sync_fd = -1, result = 1;

	if (gpu_device_open(&producer, context) || gpu_device_open(&consumer, context))
		goto out;
	if (gpu_image_create(&producer, &source, WIDTH, HEIGHT, 0) ||
	    gpu_image_export(&producer, &source, &description, &memory_fd) ||
	    gpu_image_import(&consumer, &imported, &description, &memory_fd) ||
	    gpu_semaphore_create(&producer, &produced) ||
	    gpu_semaphore_create(&consumer, &acquired) || readback_create(&consumer, &readback))
		goto out;
	if (gpu_commands_begin(&producer, &commands))
		goto out;
	whole_image_barrier(commands, (VkImageMemoryBarrier) {
		.image = source.handle,
		.oldLayout = VK_IMAGE_LAYOUT_UNDEFINED,
		.newLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
		.srcAccessMask = 0,
		.dstAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT,
		.srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
		.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
	});
	vkCmdClearColorImage(commands, source.handle, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
			     &red, 1, &color_range);
	whole_image_barrier(commands, (VkImageMemoryBarrier) {
		.image = source.handle,
		.oldLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
		.newLayout = VK_IMAGE_LAYOUT_GENERAL,
		.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT,
		.dstAccessMask = 0,
		.srcQueueFamilyIndex = context->queue_family,
		.dstQueueFamilyIndex = VK_QUEUE_FAMILY_EXTERNAL,
	});
	if (vkEndCommandBuffer(commands) != VK_SUCCESS || atomic_load(&context->validation_errors))
		goto out;
	submit.commandBufferCount = 1;
	submit.pCommandBuffers = &commands;
	submit.signalSemaphoreCount = 1;
	submit.pSignalSemaphores = &produced;
	if (vkQueueSubmit(producer.queue, 1, &submit, VK_NULL_HANDLE) != VK_SUCCESS ||
	    gpu_semaphore_export(&producer, produced, &sync_fd))
		goto out;
	printf("submitted producer: sync_fd=%s\n", sync_fd == -1 ? "already complete" : "exported");
	if (gpu_semaphore_import(&consumer, acquired, sync_fd))
		goto out;
	sync_fd = -1;
	if (gpu_commands_begin(&consumer, &commands))
		goto out;
	whole_image_barrier(commands, (VkImageMemoryBarrier) {
		.image = imported.handle,
		.oldLayout = VK_IMAGE_LAYOUT_GENERAL,
		.newLayout = VK_IMAGE_LAYOUT_GENERAL,
		.srcAccessMask = 0,
		.dstAccessMask = VK_ACCESS_TRANSFER_READ_BIT,
		.srcQueueFamilyIndex = VK_QUEUE_FAMILY_EXTERNAL,
		.dstQueueFamilyIndex = context->queue_family,
	});
	whole_image_barrier(commands, (VkImageMemoryBarrier) {
		.image = imported.handle,
		.oldLayout = VK_IMAGE_LAYOUT_GENERAL,
		.newLayout = VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
		.srcAccessMask = VK_ACCESS_TRANSFER_READ_BIT,
		.dstAccessMask = VK_ACCESS_TRANSFER_READ_BIT,
		.srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
		.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
	});
	vkCmdCopyImageToBuffer(commands, imported.handle, VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
			       readback.buffer, 1, &copy);
	vkCmdPipelineBarrier(commands, VK_PIPELINE_STAGE_TRANSFER_BIT, VK_PIPELINE_STAGE_HOST_BIT,
			     0, 1, &host, 0, NULL, 0, NULL);
	if (vkEndCommandBuffer(commands) != VK_SUCCESS || atomic_load(&context->validation_errors))
		goto out;
	submit.waitSemaphoreCount = 1;
	submit.pWaitSemaphores = &acquired;
	submit.pWaitDstStageMask = &stage;
	submit.signalSemaphoreCount = 0;
	submit.pSignalSemaphores = NULL;
	if (vkQueueSubmit(consumer.queue, 1, &submit, VK_NULL_HANDLE) != VK_SUCCESS ||
	    vkQueueWaitIdle(consumer.queue) != VK_SUCCESS || check_pixels(&consumer, &readback))
		goto out;
	result = 0;
out:
	/* A failed test still owns resources referenced by any accepted GPU submissions. */
	if (consumer.handle && vkDeviceWaitIdle(consumer.handle) != VK_SUCCESS)
		result = 1;
	if (producer.handle && vkDeviceWaitIdle(producer.handle) != VK_SUCCESS)
		result = 1;
	if (memory_fd >= 0)
		close(memory_fd);
	if (sync_fd >= 0)
		close(sync_fd);
	if (consumer.handle) {
		vkDestroyBuffer(consumer.handle, readback.buffer, NULL);
		vkFreeMemory(consumer.handle, readback.memory, NULL);
		vkDestroySemaphore(consumer.handle, acquired, NULL);
	}
	if (producer.handle)
		vkDestroySemaphore(producer.handle, produced, NULL);
	gpu_image_destroy(&consumer, &imported);
	gpu_image_destroy(&producer, &source);
	if (gpu_device_close(&consumer))
		result = 1;
	if (gpu_device_close(&producer))
		result = 1;
	return result;
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
	result = handoff(&context);
	gpu_context_close(&context);
	if (atomic_load(&context.validation_errors))
		result = 1;
	if (!result)
		puts("PASS: generated GPU image imported on a separate Vulkan device; all pixels match");
	return result;
}
