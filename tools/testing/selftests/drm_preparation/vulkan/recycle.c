// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <stdio.h>
#include <string.h>
#include <unistd.h>

#include "recycle.h"
#include "sync_file.h"

#define IMAGE_SIZE 256
#define MAX_IMAGES 2

static const VkImageSubresourceRange color_range = {
	.aspectMask = VK_IMAGE_ASPECT_COLOR_BIT,
	.levelCount = 1,
	.layerCount = 1,
};

/* Per-operation native resources are reclaimed, not accumulated in the device pool. */
struct job {
	struct gpu_device *device;
	VkCommandBuffer commands;
	VkSemaphore waits[MAX_IMAGES];
	unsigned int wait_count;
	VkSemaphore completed;
	VkFence fence;
	int submitted;
	struct gpu_recycle_image *released[MAX_IMAGES];
	unsigned int release_count;
};

static void barrier(struct job *job, VkImage image, VkImageLayout old_layout,
		    uint32_t from, uint32_t to, VkAccessFlags source, VkAccessFlags destination)
{
	VkImageMemoryBarrier barrier = {
		.sType = VK_STRUCTURE_TYPE_IMAGE_MEMORY_BARRIER,
		.image = image,
		.oldLayout = old_layout,
		.newLayout = VK_IMAGE_LAYOUT_GENERAL,
		.srcAccessMask = source,
		.dstAccessMask = destination,
		.srcQueueFamilyIndex = from,
		.dstQueueFamilyIndex = to,
		.subresourceRange = color_range,
	};

	vkCmdPipelineBarrier(job->commands, VK_PIPELINE_STAGE_ALL_COMMANDS_BIT,
			     VK_PIPELINE_STAGE_ALL_COMMANDS_BIT, 0, 0, NULL, 0, NULL, 1, &barrier);
}

static int job_begin(struct job *job, struct gpu_device *device)
{
	VkFenceCreateInfo fence = { .sType = VK_STRUCTURE_TYPE_FENCE_CREATE_INFO };

	job->device = device;
	if (gpu_commands_begin(device, &job->commands) ||
	    gpu_semaphore_create(device, &job->completed) ||
	    vkCreateFence(device->handle, &fence, NULL, &job->fence) != VK_SUCCESS)
		return -1;
	return 0;
}

static void job_destroy(struct job *job)
{
	unsigned int i;

	if (!job->device)
		return;
	/* A timeout diagnoses failure; it does not cancel accepted commands. */
	if (job->submitted)
		vkDeviceWaitIdle(job->device->handle);
	for (i = 0; i < job->wait_count; i++)
		vkDestroySemaphore(job->device->handle, job->waits[i], NULL);
	vkDestroySemaphore(job->device->handle, job->completed, NULL);
	vkDestroyFence(job->device->handle, job->fence, NULL);
	if (job->commands)
		vkFreeCommandBuffers(job->device->handle, job->device->pool, 1, &job->commands);
}

static int job_finish(struct job *job)
{
	VkPipelineStageFlags stages[MAX_IMAGES] = {
		VK_PIPELINE_STAGE_ALL_COMMANDS_BIT, VK_PIPELINE_STAGE_ALL_COMMANDS_BIT,
	};
	VkSubmitInfo submit = {
		.sType = VK_STRUCTURE_TYPE_SUBMIT_INFO,
		.commandBufferCount = 1,
		.pCommandBuffers = &job->commands,
		.waitSemaphoreCount = job->wait_count,
		.pWaitSemaphores = job->waits,
		.pWaitDstStageMask = stages,
		.signalSemaphoreCount = 1,
		.pSignalSemaphores = &job->completed,
	};
	unsigned int i;
	int fd = -1, result = -1;

	if (vkEndCommandBuffer(job->commands) != VK_SUCCESS ||
	    atomic_load(&job->device->context->validation_errors) ||
	    vkQueueSubmit(job->device->queue, 1, &submit, job->fence) != VK_SUCCESS)
		return -1;
	job->submitted = 1;
	if (gpu_semaphore_export(job->device, job->completed, &fd) ||
	    gpu_sync_file_check(fd, 5000) ||
	    vkWaitForFences(job->device->handle, 1, &job->fence, VK_TRUE, 5000000000ULL) != VK_SUCCESS)
		goto out;
	job->submitted = 0;
	for (i = 0; i < job->release_count; i++) {
		struct gpu_recycle_image *image = job->released[i];

		image->completion_fd = fd < 0 ? -1 : dup(fd);
		if (fd >= 0 && image->completion_fd < 0)
			goto out;
	}
	result = 0;
out:
	if (fd >= 0)
		close(fd);
	return result;
}

static int acquire(struct job *job, struct gpu_recycle_image *image, unsigned int side)
{
	VkAccessFlags access = VK_ACCESS_TRANSFER_READ_BIT | VK_ACCESS_TRANSFER_WRITE_BIT;

	if (side > 1 || image->owner != side || image->devices[side] != job->device)
		return -1;
	if (image->external) {
		VkSemaphore *wait;

		if (job->wait_count == MAX_IMAGES)
			return -1;
		wait = &job->waits[job->wait_count++];
		if (gpu_semaphore_create(job->device, wait) ||
		    gpu_semaphore_import(job->device, *wait, image->completion_fd))
			return -1;
		image->completion_fd = -1;
		barrier(job, image->images[side].handle, VK_IMAGE_LAYOUT_GENERAL,
			VK_QUEUE_FAMILY_EXTERNAL, job->device->context->queue_family, 0, access);
		image->external = 0;
	} else {
		barrier(job, image->images[side].handle,
			image->initialized ? VK_IMAGE_LAYOUT_GENERAL : VK_IMAGE_LAYOUT_UNDEFINED,
			VK_QUEUE_FAMILY_IGNORED, VK_QUEUE_FAMILY_IGNORED,
			image->initialized ? access : 0, access);
	}
	image->initialized = 1;
	return 0;
}

static int release(struct job *job, struct gpu_recycle_image *image)
{
	if (job->release_count == MAX_IMAGES || image->external ||
	    image->devices[image->owner] != job->device)
		return -1;
	barrier(job, image->images[image->owner].handle, VK_IMAGE_LAYOUT_GENERAL,
		job->device->context->queue_family, VK_QUEUE_FAMILY_EXTERNAL,
		VK_ACCESS_TRANSFER_READ_BIT | VK_ACCESS_TRANSFER_WRITE_BIT, 0);
	image->owner ^= 1;
	image->external = 1;
	job->released[job->release_count++] = image;
	return 0;
}

int gpu_recycle_create(struct gpu_recycle_image *image, struct gpu_device *writer,
		       struct gpu_device *reader, uint64_t modifier)
{
	struct gpu_image_description description;
	int fd = -1, result = -1;

	*image = (struct gpu_recycle_image) {
		.devices = { writer, reader },
		.completion_fd = -1,
	};
	if (gpu_image_create(writer, &image->images[0], IMAGE_SIZE, IMAGE_SIZE, modifier) ||
	    gpu_image_export(writer, &image->images[0], &description, &fd) ||
	    gpu_image_import(reader, &image->images[1], &description, &fd))
		goto out;
	result = 0;
out:
	if (fd >= 0)
		close(fd);
	return result;
}

void gpu_recycle_destroy(struct gpu_recycle_image *image)
{
	unsigned int i;

	/* An unconstructed slot has no devices and owns no descriptor. */
	if (!image->devices[0])
		return;
	if (image->completion_fd >= 0)
		close(image->completion_fd);
	for (i = 0; i < 2; i++)
		gpu_image_destroy(image->devices[i], &image->images[i]);
	memset(image, 0, sizeof(*image));
}

static void blit(struct job *job, VkImage source, VkImage destination)
{
	VkImageBlit region = {
		.srcSubresource = { .aspectMask = VK_IMAGE_ASPECT_COLOR_BIT, .layerCount = 1 },
		.srcOffsets = { { 0, 0, 0 }, { IMAGE_SIZE, IMAGE_SIZE, 1 } },
		.dstSubresource = { .aspectMask = VK_IMAGE_ASPECT_COLOR_BIT, .layerCount = 1 },
		.dstOffsets = { { 0, 0, 0 }, { IMAGE_SIZE, IMAGE_SIZE, 1 } },
	};

	vkCmdBlitImage(job->commands, source, VK_IMAGE_LAYOUT_GENERAL,
		       destination, VK_IMAGE_LAYOUT_GENERAL, 1, &region, VK_FILTER_NEAREST);
}

int gpu_recycle_source(struct gpu_recycle_image *staging, unsigned int color, uint64_t modifier)
{
	struct gpu_device *device = staging->devices[0];
	struct gpu_image source = { 0 };
	struct job job = { 0 };
	VkClearColorValue value = {
		.float32 = { !!(color & 1), !!(color & 2), !!(color & 4), 1.0f },
	};
	int result = -1;

	if (gpu_image_create(device, &source, IMAGE_SIZE, IMAGE_SIZE, modifier) ||
	    job_begin(&job, device) || acquire(&job, staging, 0))
		goto out;
	barrier(&job, source.handle, VK_IMAGE_LAYOUT_UNDEFINED,
		VK_QUEUE_FAMILY_IGNORED, VK_QUEUE_FAMILY_IGNORED, 0, VK_ACCESS_TRANSFER_WRITE_BIT);
	vkCmdClearColorImage(job.commands, source.handle, VK_IMAGE_LAYOUT_GENERAL, &value, 1, &color_range);
	barrier(&job, source.handle, VK_IMAGE_LAYOUT_GENERAL,
		VK_QUEUE_FAMILY_IGNORED, VK_QUEUE_FAMILY_IGNORED,
		VK_ACCESS_TRANSFER_WRITE_BIT, VK_ACCESS_TRANSFER_READ_BIT);
	blit(&job, source.handle, staging->images[0].handle);
	if (release(&job, staging))
		goto out;
	result = job_finish(&job);
out:
	job_destroy(&job);
	gpu_image_destroy(device, &source);
	return result;
}

int gpu_recycle_copy(struct gpu_recycle_image *staging, struct gpu_recycle_image *output)
{
	struct job job = { 0 };
	int result = -1;

	if (job_begin(&job, staging->devices[1]) || acquire(&job, staging, 1) ||
	    acquire(&job, output, 0))
		goto out;
	blit(&job, staging->images[1].handle, output->images[0].handle);
	if (release(&job, staging) || release(&job, output))
		goto out;
	result = job_finish(&job);
out:
	job_destroy(&job);
	return result;
}

int gpu_recycle_return(struct gpu_recycle_image *output)
{
	struct job job = { 0 };
	int result = -1;

	if (job_begin(&job, output->devices[1]) || acquire(&job, output, 1) || release(&job, output))
		goto out;
	result = job_finish(&job);
out:
	job_destroy(&job);
	return result;
}

int gpu_recycle_check(struct gpu_recycle_image *output, unsigned int color)
{
	struct gpu_device *device = output->devices[1];
	struct job job = { 0 };
	VkBuffer buffer = VK_NULL_HANDLE;
	VkDeviceMemory memory = VK_NULL_HANDLE;
	VkBufferCreateInfo info = {
		.sType = VK_STRUCTURE_TYPE_BUFFER_CREATE_INFO,
		.size = IMAGE_SIZE * IMAGE_SIZE * 4,
		.usage = VK_BUFFER_USAGE_TRANSFER_DST_BIT,
		.sharingMode = VK_SHARING_MODE_EXCLUSIVE,
	};
	VkMemoryAllocateInfo allocation = { .sType = VK_STRUCTURE_TYPE_MEMORY_ALLOCATE_INFO };
	VkMemoryRequirements requirements;
	VkBufferImageCopy copy = {
		.imageSubresource = { .aspectMask = VK_IMAGE_ASPECT_COLOR_BIT, .layerCount = 1 },
		.imageExtent = { IMAGE_SIZE, IMAGE_SIZE, 1 },
	};
	VkMemoryBarrier host = {
		.sType = VK_STRUCTURE_TYPE_MEMORY_BARRIER,
		.srcAccessMask = VK_ACCESS_TRANSFER_WRITE_BIT,
		.dstAccessMask = VK_ACCESS_HOST_READ_BIT,
	};
	const unsigned char expected[4] = {
		(color & 4) ? 255 : 0, (color & 2) ? 255 : 0, (color & 1) ? 255 : 0, 255,
	};
	unsigned char *pixels;
	unsigned int i;
	int result = -1;

	if (vkCreateBuffer(device->handle, &info, NULL, &buffer) != VK_SUCCESS)
		goto out;
	vkGetBufferMemoryRequirements(device->handle, buffer, &requirements);
	allocation.allocationSize = requirements.size;
	if (gpu_memory_type(device, requirements.memoryTypeBits,
			    VK_MEMORY_PROPERTY_HOST_VISIBLE_BIT | VK_MEMORY_PROPERTY_HOST_COHERENT_BIT,
			    &allocation.memoryTypeIndex) ||
	    vkAllocateMemory(device->handle, &allocation, NULL, &memory) != VK_SUCCESS ||
	    vkBindBufferMemory(device->handle, buffer, memory, 0) != VK_SUCCESS ||
	    job_begin(&job, device) || acquire(&job, output, 1))
		goto out;
	vkCmdCopyImageToBuffer(job.commands, output->images[1].handle, VK_IMAGE_LAYOUT_GENERAL,
			       buffer, 1, &copy);
	vkCmdPipelineBarrier(job.commands, VK_PIPELINE_STAGE_TRANSFER_BIT, VK_PIPELINE_STAGE_HOST_BIT,
			     0, 1, &host, 0, NULL, 0, NULL);
	if (job_finish(&job) ||
	    vkMapMemory(device->handle, memory, 0, VK_WHOLE_SIZE, 0, (void **)&pixels) != VK_SUCCESS)
		goto out;
	result = 0;
	for (i = 0; i < IMAGE_SIZE * IMAGE_SIZE; i++) {
		if (memcmp(pixels + 4 * i, expected, 4)) {
			fprintf(stderr, "Held image color %u mismatch at pixel %u\n", color, i);
			result = -1;
			break;
		}
	}
	vkUnmapMemory(device->handle, memory);
out:
	job_destroy(&job);
	vkDestroyBuffer(device->handle, buffer, NULL);
	vkFreeMemory(device->handle, memory, NULL);
	return result;
}
