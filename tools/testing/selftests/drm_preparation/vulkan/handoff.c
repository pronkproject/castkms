// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <errno.h>
#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "image.h"
#include "sync_file.h"

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

static int check_pixels(struct gpu_device *device, struct readback *readback, unsigned int frame)
{
	const unsigned char expected[] = {
		(frame & 4) ? 255 : 0,
		(frame & 2) ? 255 : 0,
		(frame & 1) ? 255 : 0,
		255,
	};
	unsigned char *pixels;
	unsigned int i;
	int result = 0;

	if (vkMapMemory(device->handle, readback->memory, 0, VK_WHOLE_SIZE, 0, (void **)&pixels) != VK_SUCCESS)
		return -1;
	for (i = 0; i < WIDTH * HEIGHT; i++) {
		/* The pixel oracle reads B8G8R8A8 bytes, not Vulkan's RGBA clear order. */
		if (memcmp(&pixels[4 * i], expected, sizeof(expected))) {
			fprintf(stderr, "Frame %u pixel %u mismatch: %u %u %u %u\n", frame, i,
				pixels[4 * i], pixels[4 * i + 1], pixels[4 * i + 2], pixels[4 * i + 3]);
			result = -1;
			break;
		}
	}
	vkUnmapMemory(device->handle, readback->memory);
	return result;
}

static int produce(struct gpu_device *producer, const struct gpu_image *source,
		   VkSemaphore produced, int *sync_fd, unsigned int frame)
{
	VkCommandBuffer commands;
	VkClearColorValue color = {
		.float32 = { !!(frame & 1), !!(frame & 2), !!(frame & 4), 1.0f },
	};
	VkSubmitInfo submit = {
		.sType = VK_STRUCTURE_TYPE_SUBMIT_INFO,
		.commandBufferCount = 1,
		.pCommandBuffers = &commands,
		.signalSemaphoreCount = 1,
		.pSignalSemaphores = &produced,
	};

	if (gpu_commands_begin(producer, &commands))
		return -1;
	whole_image_barrier(commands, (VkImageMemoryBarrier) {
		.image = source->handle,
		.oldLayout = VK_IMAGE_LAYOUT_UNDEFINED,
		.newLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
		.srcAccessMask = 0,
		.dstAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT,
		.srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
		.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
	});
	vkCmdClearColorImage(commands, source->handle, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
			     &color, 1, &color_range);
	whole_image_barrier(commands, (VkImageMemoryBarrier) {
		.image = source->handle,
		.oldLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
		.newLayout = VK_IMAGE_LAYOUT_GENERAL,
		.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT,
		.dstAccessMask = 0,
		.srcQueueFamilyIndex = producer->context->queue_family,
		.dstQueueFamilyIndex = VK_QUEUE_FAMILY_EXTERNAL,
	});
	if (vkEndCommandBuffer(commands) != VK_SUCCESS ||
	    atomic_load(&producer->context->validation_errors) ||
	    vkQueueSubmit(producer->queue, 1, &submit, VK_NULL_HANDLE) != VK_SUCCESS)
		return -1;
	return gpu_semaphore_export(producer, produced, sync_fd);
}

static int begin_blit(struct gpu_device *device, const struct gpu_image *input,
		      const struct gpu_image *output, VkCommandBuffer *commands)
{
	VkImageBlit region = {
		.srcSubresource = { .aspectMask = VK_IMAGE_ASPECT_COLOR_BIT, .layerCount = 1 },
		.srcOffsets = { { 0, 0, 0 }, { WIDTH, HEIGHT, 1 } },
		.dstSubresource = { .aspectMask = VK_IMAGE_ASPECT_COLOR_BIT, .layerCount = 1 },
		.dstOffsets = { { 0, 0, 0 }, { WIDTH, HEIGHT, 1 } },
	};

	if (gpu_commands_begin(device, commands))
		return -1;
	whole_image_barrier(*commands, (VkImageMemoryBarrier) {
		.image = input->handle,
		.oldLayout = VK_IMAGE_LAYOUT_GENERAL,
		.newLayout = VK_IMAGE_LAYOUT_GENERAL,
		.srcAccessMask = 0,
		.dstAccessMask = VK_ACCESS_TRANSFER_READ_BIT,
		.srcQueueFamilyIndex = VK_QUEUE_FAMILY_EXTERNAL,
		.dstQueueFamilyIndex = device->context->queue_family,
	});
	whole_image_barrier(*commands, (VkImageMemoryBarrier) {
		.image = input->handle,
		.oldLayout = VK_IMAGE_LAYOUT_GENERAL,
		.newLayout = VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
		.srcAccessMask = VK_ACCESS_TRANSFER_READ_BIT,
		.dstAccessMask = VK_ACCESS_TRANSFER_READ_BIT,
		.srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
		.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
	});
	whole_image_barrier(*commands, (VkImageMemoryBarrier) {
		.image = output->handle,
		.oldLayout = VK_IMAGE_LAYOUT_UNDEFINED,
		.newLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
		.srcAccessMask = 0,
		.dstAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT,
		.srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
		.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
	});
	vkCmdBlitImage(*commands, input->handle, VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
		       output->handle, VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL, 1, &region, VK_FILTER_NEAREST);
	return 0;
}

static int submit_blit(struct gpu_device *device, VkCommandBuffer commands,
		       VkSemaphore acquired, VkSemaphore completed)
{
	VkPipelineStageFlags stage = VK_PIPELINE_STAGE_ALL_COMMANDS_BIT;
	VkSubmitInfo submit = {
		.sType = VK_STRUCTURE_TYPE_SUBMIT_INFO,
		.commandBufferCount = 1,
		.pCommandBuffers = &commands,
		.waitSemaphoreCount = 1,
		.pWaitSemaphores = &acquired,
		.pWaitDstStageMask = &stage,
		.signalSemaphoreCount = completed ? 1 : 0,
		.pSignalSemaphores = completed ? &completed : NULL,
	};

	if (vkEndCommandBuffer(commands) != VK_SUCCESS || atomic_load(&device->context->validation_errors))
		return -1;
	return vkQueueSubmit(device->queue, 1, &submit, VK_NULL_HANDLE) == VK_SUCCESS ? 0 : -1;
}

static void copy_to_readback(VkCommandBuffer commands, const struct gpu_image *image,
			     const struct readback *readback)
{
	VkBufferImageCopy copy = {
		.imageSubresource = { .aspectMask = VK_IMAGE_ASPECT_COLOR_BIT, .layerCount = 1 },
		.imageExtent = { WIDTH, HEIGHT, 1 },
	};
	VkMemoryBarrier host = {
		.sType = VK_STRUCTURE_TYPE_MEMORY_BARRIER,
		.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT,
		.dstAccessMask = VK_ACCESS_HOST_READ_BIT,
	};

	whole_image_barrier(commands, (VkImageMemoryBarrier) {
		.image = image->handle,
		.oldLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
		.newLayout = VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
		.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT,
		.dstAccessMask = VK_ACCESS_TRANSFER_READ_BIT,
		.srcQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
		.dstQueueFamilyIndex = VK_QUEUE_FAMILY_IGNORED,
	});
	vkCmdCopyImageToBuffer(commands, image->handle, VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
			       readback->buffer, 1, &copy);
	vkCmdPipelineBarrier(commands, VK_PIPELINE_STAGE_TRANSFER_BIT, VK_PIPELINE_STAGE_HOST_BIT,
			     0, 1, &host, 0, NULL, 0, NULL);
}

static int share_image(struct gpu_device *owner, const struct gpu_image *image,
		       struct gpu_device *reader, struct gpu_image *imported)
{
	struct gpu_image_description description;
	int fd = -1, result;

	result = gpu_image_export(owner, image, &description, &fd);
	if (!result)
		result = gpu_image_import(reader, imported, &description, &fd);
	if (fd >= 0)
		close(fd);
	return result;
}

static int handoff(struct gpu_context *context, unsigned int frame, uint64_t modifier)
{
	struct gpu_device producer = { 0 }, source_worker = { 0 }, output_worker = { 0 };
	struct gpu_image source = { 0 }, source_import = { 0 };
	struct gpu_image staging = { 0 }, staging_import = { 0 }, output = { 0 };
	struct gpu_image_description output_description;
	struct readback readback = { 0 };
	VkSemaphore produced = VK_NULL_HANDLE, source_acquired = VK_NULL_HANDLE;
	VkSemaphore source_completed = VK_NULL_HANDLE, output_acquired = VK_NULL_HANDLE;
	VkSemaphore output_completed = VK_NULL_HANDLE;
	VkCommandBuffer commands;
	int sync_fd = -1, output_fd = -1, result = 1;

	if (gpu_device_open(&producer, context) || gpu_device_open(&source_worker, context) ||
	    gpu_device_open(&output_worker, context))
		goto out;
	if (gpu_image_create(&producer, &source, WIDTH, HEIGHT, modifier) ||
	    share_image(&producer, &source, &source_worker, &source_import) ||
	    gpu_image_create(&source_worker, &staging, WIDTH, HEIGHT, modifier) ||
	    share_image(&source_worker, &staging, &output_worker, &staging_import) ||
	    gpu_image_create(&output_worker, &output, WIDTH, HEIGHT, modifier) ||
	    gpu_image_export(&output_worker, &output, &output_description, &output_fd) ||
	    gpu_semaphore_create(&producer, &produced) ||
	    gpu_semaphore_create(&source_worker, &source_acquired) ||
	    gpu_semaphore_create(&source_worker, &source_completed) ||
	    gpu_semaphore_create(&output_worker, &output_acquired) ||
	    gpu_semaphore_create(&output_worker, &output_completed) ||
	    readback_create(&output_worker, &readback))
		goto out;
	if (produce(&producer, &source, produced, &sync_fd, frame) ||
	    gpu_semaphore_import(&source_worker, source_acquired, sync_fd))
		goto out;
	sync_fd = -1;
	if (begin_blit(&source_worker, &source_import, &staging, &commands))
		goto out;
	whole_image_barrier(commands, (VkImageMemoryBarrier) {
		.image = staging.handle,
		.oldLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
		.newLayout = VK_IMAGE_LAYOUT_GENERAL,
		.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT,
		.dstAccessMask = 0,
		.srcQueueFamilyIndex = context->queue_family,
		.dstQueueFamilyIndex = VK_QUEUE_FAMILY_EXTERNAL,
	});
	if (submit_blit(&source_worker, commands, source_acquired, source_completed) ||
	    gpu_semaphore_export(&source_worker, source_completed, &sync_fd))
		goto out;
	printf("submitted A-to-E: sync_fd=%s\n", sync_fd == -1 ? "already complete" : "exported");
	if (gpu_sync_file_check(sync_fd, 5000) ||
	    vkDeviceWaitIdle(source_worker.handle) != VK_SUCCESS ||
	    vkDeviceWaitIdle(producer.handle) != VK_SUCCESS)
		goto out;
	gpu_image_destroy(&source_worker, &source_import);
	gpu_image_destroy(&producer, &source);
	puts("source A destroyed before E-to-D submission");
	if (gpu_semaphore_import(&output_worker, output_acquired, sync_fd))
		goto out;
	sync_fd = -1;
	if (begin_blit(&output_worker, &staging_import, &output, &commands))
		goto out;
	copy_to_readback(commands, &output, &readback);
	whole_image_barrier(commands, (VkImageMemoryBarrier) {
		.image = output.handle,
		.oldLayout = VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
		.newLayout = VK_IMAGE_LAYOUT_GENERAL,
		.srcAccessMask = VK_ACCESS_TRANSFER_READ_BIT | VK_ACCESS_TRANSFER_WRITE_BIT,
		.dstAccessMask = 0,
		.srcQueueFamilyIndex = context->queue_family,
		.dstQueueFamilyIndex = VK_QUEUE_FAMILY_EXTERNAL,
	});
	if (submit_blit(&output_worker, commands, output_acquired, output_completed) ||
	    gpu_semaphore_export(&output_worker, output_completed, &sync_fd) ||
	    gpu_sync_file_check(sync_fd, 5000) ||
	    vkQueueWaitIdle(output_worker.queue) != VK_SUCCESS || check_pixels(&output_worker, &readback, frame))
		goto out;
	result = 0;
out:
	/* A failed test still owns resources referenced by any accepted GPU submissions. */
	if (output_worker.handle && vkDeviceWaitIdle(output_worker.handle) != VK_SUCCESS)
		result = 1;
	if (source_worker.handle && vkDeviceWaitIdle(source_worker.handle) != VK_SUCCESS)
		result = 1;
	if (producer.handle && vkDeviceWaitIdle(producer.handle) != VK_SUCCESS)
		result = 1;
	if (output_fd >= 0)
		close(output_fd);
	if (sync_fd >= 0)
		close(sync_fd);
	if (output_worker.handle) {
		vkDestroyBuffer(output_worker.handle, readback.buffer, NULL);
		vkFreeMemory(output_worker.handle, readback.memory, NULL);
		vkDestroySemaphore(output_worker.handle, output_acquired, NULL);
		vkDestroySemaphore(output_worker.handle, output_completed, NULL);
	}
	if (source_worker.handle) {
		vkDestroySemaphore(source_worker.handle, source_acquired, NULL);
		vkDestroySemaphore(source_worker.handle, source_completed, NULL);
	}
	if (producer.handle)
		vkDestroySemaphore(producer.handle, produced, NULL);
	gpu_image_destroy(&output_worker, &output);
	gpu_image_destroy(&output_worker, &staging_import);
	gpu_image_destroy(&source_worker, &staging);
	gpu_image_destroy(&source_worker, &source_import);
	gpu_image_destroy(&producer, &source);
	if (gpu_device_close(&output_worker))
		result = 1;
	if (gpu_device_close(&source_worker))
		result = 1;
	if (gpu_device_close(&producer))
		result = 1;
	return result;
}

int main(int argc, char **argv)
{
	struct gpu_context context;
	unsigned int frame;
	uint64_t modifier = 0;
	int result, i, validation = 0, modifier_set = 0;

	if (argc < 2)
		goto usage;
	for (i = 2; i < argc; i++) {
		if (!strcmp(argv[i], "--validation") && !validation) {
			validation = 1;
		} else if (!strcmp(argv[i], "--modifier") && !modifier_set && i + 1 < argc) {
			unsigned long long value;
			char *end;

			i++;
			if (argv[i][0] < '0' || argv[i][0] > '9')
				goto usage;
			errno = 0;
			value = strtoull(argv[i], &end, 0);
			if (errno || end == argv[i] || *end || value > UINT64_MAX)
				goto usage;
			modifier = value;
			modifier_set = 1;
		} else {
			goto usage;
		}
	}
	result = gpu_context_open(&context, argv[1], validation);
	if (result)
		return result == -ENODEV ? 4 : 1;
	printf("requested_modifier=0x%016" PRIx64 "\n", modifier);
	for (frame = 0; frame < 8; frame++) {
		printf("frame=%u\n", frame);
		result = handoff(&context, frame, modifier);
		if (result || atomic_load(&context.validation_errors))
			break;
	}
	gpu_context_close(&context);
	if (atomic_load(&context.validation_errors))
		result = 1;
	if (!result)
		puts("PASS: eight changing A-to-E-to-D images match after source destruction");
	return result;
usage:
	fprintf(stderr, "Usage: %s RENDER_NODE [--validation] [--modifier INTEGER]\n", argv[0]);
	return 1;
}
