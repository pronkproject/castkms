// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <errno.h>
#include <inttypes.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "image.h"
#include "sync_file.h"

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
	VkExtent2D extent;
};

static int readback_create(struct gpu_device *device, struct readback *readback,
			   VkExtent2D extent)
{
	VkBufferCreateInfo info = {
		.sType = VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO,
		.size = (VkDeviceSize)extent.width * extent.height * 4,
		.usage = VK_BUFFER_USAGE_TRANSFER_DST_BIT,
		.sharingMode = VK_SHARING_MODE_EXCLUSIVE,
	};
	VkMemoryAllocateInfo allocation = { .sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO };
	VkMemoryRequirements requirements;

	readback->extent = extent;
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
	size_t i, count = (size_t)readback->extent.width * readback->extent.height;
	int result = 0;

	if (vkMapMemory(device->handle, readback->memory, 0, VK_WHOLE_SIZE, 0, (void **)&pixels) != VK_SUCCESS)
		return -1;
	for (i = 0; i < count; i++) {
		/* The pixel oracle reads B8G8R8A8 bytes, not Vulkan's RGBA clear order. */
		if (memcmp(&pixels[4 * i], expected, sizeof(expected))) {
			fprintf(stderr, "Frame %u pixel %zu mismatch: %u %u %u %u\n", frame, i,
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
		.srcOffsets = { { 0, 0, 0 }, { input->width, input->height, 1 } },
		.dstSubresource = { .aspectMask = VK_IMAGE_ASPECT_COLOR_BIT, .layerCount = 1 },
		.dstOffsets = { { 0, 0, 0 }, { output->width, output->height, 1 } },
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
		.imageExtent = { image->width, image->height, 1 },
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

#define FRAME_COUNT 8

struct workers {
	struct gpu_device producer;
	struct gpu_device source;
	struct gpu_device output;
};

struct frame {
	struct gpu_image source;
	struct gpu_image source_import;
	struct gpu_image staging;
	struct gpu_image staging_import;
	struct gpu_image output;
	struct readback readback;
	VkSemaphore produced;
	VkSemaphore source_acquired;
	VkSemaphore source_completed;
	VkSemaphore output_acquired;
	VkSemaphore output_completed;
	int source_fence_fd;
	int output_fence_fd;
	int output_memory_fd;
};

static int frame_create(struct workers *workers, struct frame *frame, uint64_t modifier,
			VkExtent2D extent)
{
	struct gpu_image_description description;

	if (gpu_image_create(&workers->producer, &frame->source, extent.width, extent.height, modifier) ||
	    share_image(&workers->producer, &frame->source, &workers->source, &frame->source_import) ||
	    gpu_image_create(&workers->source, &frame->staging, extent.width, extent.height, modifier) ||
	    share_image(&workers->source, &frame->staging, &workers->output, &frame->staging_import) ||
	    gpu_image_create(&workers->output, &frame->output, extent.width, extent.height, modifier) ||
	    gpu_image_export(&workers->output, &frame->output, &description, &frame->output_memory_fd) ||
	    gpu_semaphore_create(&workers->producer, &frame->produced) ||
	    gpu_semaphore_create(&workers->source, &frame->source_acquired) ||
	    gpu_semaphore_create(&workers->source, &frame->source_completed) ||
	    gpu_semaphore_create(&workers->output, &frame->output_acquired) ||
	    gpu_semaphore_create(&workers->output, &frame->output_completed) ||
	    readback_create(&workers->output, &frame->readback, extent))
		return -1;
	return 0;
}

static int submit_source(struct workers *workers, struct frame *frame, unsigned int index)
{
	VkCommandBuffer commands;
	int producer_fd = -1, result = -1;

	if (produce(&workers->producer, &frame->source, frame->produced, &producer_fd, index) ||
	    gpu_semaphore_import(&workers->source, frame->source_acquired, producer_fd))
		goto out;
	producer_fd = -1;
	if (begin_blit(&workers->source, &frame->source_import, &frame->staging, &commands))
		goto out;
	whole_image_barrier(commands, (VkImageMemoryBarrier) {
		.image = frame->staging.handle,
		.oldLayout = VK_IMAGE_LAYOUT_TRANSFER_DST_OPTIMAL,
		.newLayout = VK_IMAGE_LAYOUT_GENERAL,
		.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT,
		.dstAccessMask = 0,
		.srcQueueFamilyIndex = workers->source.context->queue_family,
		.dstQueueFamilyIndex = VK_QUEUE_FAMILY_EXTERNAL,
	});
	if (submit_blit(&workers->source, commands, frame->source_acquired, frame->source_completed) ||
	    gpu_semaphore_export(&workers->source, frame->source_completed, &frame->source_fence_fd))
		goto out;
	result = 0;
out:
	if (producer_fd >= 0)
		close(producer_fd);
	return result;
}

static int submit_output(struct workers *workers, struct frame *frame, int foreign_output)
{
	VkCommandBuffer commands;

	if (gpu_semaphore_import(&workers->output, frame->output_acquired, frame->source_fence_fd))
		return -1;
	frame->source_fence_fd = -1;
	if (begin_blit(&workers->output, &frame->staging_import, &frame->output, &commands))
		return -1;
	copy_to_readback(commands, &frame->output, &frame->readback);
	whole_image_barrier(commands, (VkImageMemoryBarrier) {
		.image = frame->output.handle,
		.oldLayout = VK_IMAGE_LAYOUT_TRANSFER_SRC_OPTIMAL,
		.newLayout = VK_IMAGE_LAYOUT_GENERAL,
		.srcAccessMask = VK_ACCESS_TRANSFER_READ_BIT | VK_ACCESS_TRANSFER_WRITE_BIT,
		.dstAccessMask = 0,
		.srcQueueFamilyIndex = workers->output.context->queue_family,
		.dstQueueFamilyIndex = foreign_output ? VK_QUEUE_FAMILY_FOREIGN_EXT : VK_QUEUE_FAMILY_EXTERNAL,
	});
	if (submit_blit(&workers->output, commands, frame->output_acquired, frame->output_completed))
		return -1;
	return gpu_semaphore_export(&workers->output, frame->output_completed, &frame->output_fence_fd);
}

/* Every device must have finished submitted uses before any frame is destroyed. */
static void frame_destroy(struct workers *workers, struct frame *frame)
{
	if (frame->source_fence_fd >= 0)
		close(frame->source_fence_fd);
	if (frame->output_fence_fd >= 0)
		close(frame->output_fence_fd);
	if (frame->output_memory_fd >= 0)
		close(frame->output_memory_fd);
	if (workers->output.handle) {
		vkDestroyBuffer(workers->output.handle, frame->readback.buffer, NULL);
		vkFreeMemory(workers->output.handle, frame->readback.memory, NULL);
		vkDestroySemaphore(workers->output.handle, frame->output_acquired, NULL);
		vkDestroySemaphore(workers->output.handle, frame->output_completed, NULL);
	}
	if (workers->source.handle) {
		vkDestroySemaphore(workers->source.handle, frame->source_acquired, NULL);
		vkDestroySemaphore(workers->source.handle, frame->source_completed, NULL);
	}
	if (workers->producer.handle)
		vkDestroySemaphore(workers->producer.handle, frame->produced, NULL);
	gpu_image_destroy(&workers->output, &frame->output);
	gpu_image_destroy(&workers->output, &frame->staging_import);
	gpu_image_destroy(&workers->source, &frame->staging);
	gpu_image_destroy(&workers->source, &frame->source_import);
	gpu_image_destroy(&workers->producer, &frame->source);
}

static int handoff(struct gpu_context *context, uint64_t modifier, int foreign_output,
		   VkExtent2D extent)
{
	struct workers workers = { 0 };
	struct frame frames[FRAME_COUNT] = { 0 };
	unsigned int i;
	int result = 1;

	/* Zero is a valid descriptor, including during partial-construction cleanup. */
	for (i = 0; i < FRAME_COUNT; i++) {
		frames[i].source_fence_fd = -1;
		frames[i].output_fence_fd = -1;
		frames[i].output_memory_fd = -1;
	}
	if (gpu_device_open(&workers.producer, context) || gpu_device_open(&workers.source, context))
		goto out;
	if (foreign_output ? gpu_device_open_foreign(&workers.output, context) :
	    gpu_device_open(&workers.output, context))
		goto out;
	for (i = 0; i < FRAME_COUNT; i++)
		if (frame_create(&workers, &frames[i], modifier, extent))
			goto out;
	for (i = 0; i < FRAME_COUNT; i++)
		if (submit_source(&workers, &frames[i], i))
			goto out;
	printf("submitted source jobs=%u before first host completion wait\n", FRAME_COUNT);
	for (i = 0; i < FRAME_COUNT; i++)
		if (gpu_sync_file_check(frames[i].source_fence_fd, 5000))
			goto out;
	if (vkDeviceWaitIdle(workers.source.handle) != VK_SUCCESS ||
	    vkDeviceWaitIdle(workers.producer.handle) != VK_SUCCESS)
		goto out;
	for (i = 0; i < FRAME_COUNT; i++) {
		gpu_image_destroy(&workers.source, &frames[i].source_import);
		gpu_image_destroy(&workers.producer, &frames[i].source);
	}
	printf("destroyed sources=%u before output submissions\n", FRAME_COUNT);
	for (i = 0; i < FRAME_COUNT; i++)
		if (submit_output(&workers, &frames[i], foreign_output))
			goto out;
	printf("submitted output jobs=%u before first output wait\n", FRAME_COUNT);
	for (i = 0; i < FRAME_COUNT; i++)
		if (gpu_sync_file_check(frames[i].output_fence_fd, 5000))
			goto out;
	if (vkQueueWaitIdle(workers.output.queue) != VK_SUCCESS)
		goto out;
	for (i = 0; i < FRAME_COUNT; i++)
		if (check_pixels(&workers.output, &frames[i].readback, i))
			goto out;
	result = 0;
out:
	if (workers.output.handle && vkDeviceWaitIdle(workers.output.handle) != VK_SUCCESS)
		result = 1;
	if (workers.source.handle && vkDeviceWaitIdle(workers.source.handle) != VK_SUCCESS)
		result = 1;
	if (workers.producer.handle && vkDeviceWaitIdle(workers.producer.handle) != VK_SUCCESS)
		result = 1;
	for (i = 0; i < FRAME_COUNT; i++)
		frame_destroy(&workers, &frames[i]);
	if (gpu_device_close(&workers.output))
		result = 1;
	if (gpu_device_close(&workers.source))
		result = 1;
	if (gpu_device_close(&workers.producer))
		result = 1;
	return result;
}

int main(int argc, char **argv)
{
	struct gpu_context context;
	VkExtent2D extent = { 256, 256 };
	uint64_t modifier = 0;
	int result, i, validation = 0, modifier_set = 0, foreign_output = 0;

	if (argc < 2)
		goto usage;
	for (i = 2; i < argc; i++) {
		if (!strcmp(argv[i], "--validation") && !validation) {
			validation = 1;
		} else if (!strcmp(argv[i], "--foreign-output") && !foreign_output) {
			foreign_output = 1;
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
	printf("output_ownership=%s\n", foreign_output ? "foreign driver" : "same Vulkan driver");
	result = handoff(&context, modifier, foreign_output, extent);
	gpu_context_close(&context);
	if (atomic_load(&context.validation_errors))
		result = 1;
	if (!result)
		puts("PASS: eight changing A-to-E-to-D images match after source destruction");
	return result;
usage:
	fprintf(stderr, "Usage: %s RENDER_NODE [--validation] [--modifier INTEGER] [--foreign-output]\n", argv[0]);
	return 1;
}
