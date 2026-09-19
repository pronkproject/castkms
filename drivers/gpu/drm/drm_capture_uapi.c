// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/capability.h>
#include <linux/dma-buf.h>
#include <linux/fcntl.h>
#include <linux/file.h>
#include <linux/sync_file.h>
#include <linux/uaccess.h>
#include <linux/user_namespace.h>

#include <drm/drm_auth.h>
#include <drm/drm_capture_completion.h>
#include <drm/drm_capture_destination.h>
#include <drm/drm_capture_grant.h>
#include <drm/drm_capture_file.h>
#include <uapi/drm/drm_capture.h>

#include "drm_capture_uapi.h"

static long capture_describe(struct file *file, void __user *arg)
{
	struct drm_capture_describe output;
	struct drm_capture_layout layout;
	struct drm_capture_description description;
	int ret;

	if (copy_from_user(&output, arg, sizeof(output)))
		return -EFAULT;
	if (output.id || output.width || output.height || output.refresh_millihz ||
	    output.mode_flags || output.max_requests || output.reserved)
		return -EINVAL;
	layout.format = output.format;
	layout.modifier = output.modifier;
	ret = drm_capture_client_describe(file, &layout, &description);
	if (ret)
		return ret;
	output.id = description.id;
	output.width = description.width;
	output.height = description.height;
	output.refresh_millihz = description.refresh_millihz;
	output.mode_flags = description.mode_flags;
	output.format = description.format;
	output.max_requests = description.max_requests;
	output.modifier = description.modifier;
	if (copy_to_user(arg, &output, sizeof(output)))
		return -EFAULT;
	return 0;
}

static long capture_create_stream(struct file *file, void __user *arg)
{
	struct drm_capture_create_stream input;

	if (copy_from_user(&input, arg, sizeof(input)))
		return -EFAULT;
	if (input.flags || input.reserved)
		return -EINVAL;
	return drm_capture_client_open_stream(file, input.id, input.offer, input.capacity);
}

static long capture_destroy_stream(struct file *file, void __user *arg)
{
	struct drm_capture_destroy_stream input;

	if (copy_from_user(&input, arg, sizeof(input)))
		return -EFAULT;
	if (input.reserved)
		return -EINVAL;
	return drm_capture_client_close_stream(file, input.id);
}

static long capture_register_destination(struct file *file, void __user *arg)
{
	struct drm_capture_register_destination input;
	struct drm_capture_destination destination = {};
	unsigned int i, previous;
	int ret;

	static_assert(ARRAY_SIZE(input.fds) == ARRAY_SIZE(destination.planes));
	if (copy_from_user(&input, arg, sizeof(input)))
		return -EFAULT;
	if (!input.id || !input.num_planes || input.num_planes > ARRAY_SIZE(input.fds) ||
	    input.flags || input.reserved[0] || input.reserved[1] || input.reserved[2])
		return -EINVAL;
	for (i = input.num_planes; i < ARRAY_SIZE(input.fds); i++) {
		if (input.fds[i] || input.strides[i] || input.offsets[i])
			return -EINVAL;
	}
	destination.width = input.width;
	destination.height = input.height;
	destination.format = input.format;
	destination.modifier = input.modifier;
	destination.num_planes = input.num_planes;
	for (i = 0; i < input.num_planes; i++) {
		struct drm_capture_destination_plane *plane = &destination.planes[i];

		/* Resolve a repeated number once, even if another task reuses that fd. */
		for (previous = 0; previous < i; previous++) {
			if (input.fds[previous] == input.fds[i])
				break;
		}
		if (previous < i) {
			plane->buffer = destination.planes[previous].buffer;
			get_dma_buf(plane->buffer);
		} else {
			plane->buffer = dma_buf_get(input.fds[i]);
			if (IS_ERR(plane->buffer)) {
				ret = PTR_ERR(plane->buffer);
				goto put_buffers;
			}
		}
		plane->stride = input.strides[i];
		plane->offset = input.offsets[i];
	}
	ret = drm_capture_client_register_destination(file, input.id, &destination);
put_buffers:
	while (i)
		dma_buf_put(destination.planes[--i].buffer);
	return ret;
}

static long capture_unregister_destination(struct file *file, void __user *arg)
{
	struct drm_capture_unregister_destination input;

	if (copy_from_user(&input, arg, sizeof(input)))
		return -EFAULT;
	if (input.reserved)
		return -EINVAL;
	return drm_capture_client_unregister_destination(file, input.id);
}

static long capture_queue_output(struct file *file, void __user *arg)
{
	struct drm_capture_queue_output input;
	struct dma_fence *reuse = NULL;
	int ret;

	if (copy_from_user(&input, arg, sizeof(input)))
		return -EFAULT;
	if (input.flags || input.reserved || input.reuse_fd < -1)
		return -EINVAL;
	if (input.reuse_fd >= 0) {
		reuse = sync_file_get_fence(input.reuse_fd);
		if (!reuse)
			return -EINVAL;
	}
	ret = drm_capture_client_queue_output(file, input.stream, input.use_id,
					      input.destination, reuse);
	dma_fence_put(reuse);
	return ret;
}

static int capture_publish_result(void *data, const struct drm_capture_completion *completion)
{
	const struct drm_capture_dequeue *input = data;
	struct drm_capture_result output = {
		.use_id = completion->use_id,
		.completed_at_ns = ktime_to_ns(completion->completed_at),
		.status = completion->status,
	};

	if (copy_to_user(u64_to_user_ptr(input->result), &output, sizeof(output)))
		return -EFAULT;
	return 0;
}

static long capture_dequeue(struct file *file, void __user *arg)
{
	struct drm_capture_dequeue input;
	struct drm_capture_completion_sink sink = {
		.publish = capture_publish_result,
		.data = &input,
	};

	if (copy_from_user(&input, arg, sizeof(input)))
		return -EFAULT;
	if (input.reserved)
		return -EINVAL;
	/* The synchronous publisher completes fallible copyout before acknowledgment. */
	return drm_capture_client_dequeue(file, input.stream, &sink);
}

static long capture_cancel(struct file *file, void __user *arg)
{
	struct drm_capture_cancel input;

	if (copy_from_user(&input, arg, sizeof(input)))
		return -EFAULT;
	if (input.reserved)
		return -EINVAL;
	return drm_capture_client_cancel(file, input.stream, input.use_id);
}

long drm_capture_client_ioctl(struct file *file, unsigned int cmd, unsigned long arg)
{
	void __user *pointer = (void __user *)arg;

	switch (cmd) {
	case DRM_IOCTL_CAPTURE_DESCRIBE:
		return capture_describe(file, pointer);
	case DRM_IOCTL_CAPTURE_CREATE_STREAM:
		return capture_create_stream(file, pointer);
	case DRM_IOCTL_CAPTURE_DESTROY_STREAM:
		return capture_destroy_stream(file, pointer);
	case DRM_IOCTL_CAPTURE_REGISTER_DESTINATION:
		return capture_register_destination(file, pointer);
	case DRM_IOCTL_CAPTURE_UNREGISTER_DESTINATION:
		return capture_unregister_destination(file, pointer);
	case DRM_IOCTL_CAPTURE_QUEUE_OUTPUT:
		return capture_queue_output(file, pointer);
	case DRM_IOCTL_CAPTURE_DEQUEUE:
		return capture_dequeue(file, pointer);
	case DRM_IOCTL_CAPTURE_CANCEL:
		return capture_cancel(file, pointer);
	default:
		return -ENOTTY;
	}
}

int drm_mode_create_capture_grant_ioctl(struct drm_device *dev, void *data,
					struct drm_file *file)
{
	const struct drm_mode_create_capture_grant *arg = data;
	struct drm_capture_target target = {
		.crtc_id = arg->crtc_id,
		.connector_id = arg->connector_id,
	};
	struct drm_capture_grant_files descriptors;
	struct drm_capture_files files = {};
	int ret;

	if (!target.crtc_id || !target.connector_id ||
	    (arg->flags & ~DRM_CAPTURE_GRANT_CREATE_ADMIN) ||
	    arg->reserved[0] || arg->reserved[1] || arg->reserved[2])
		return -EINVAL;
	if ((arg->flags & DRM_CAPTURE_GRANT_CREATE_ADMIN) &&
	    !ns_capable(&init_user_ns, CAP_SYS_ADMIN))
		return -EACCES;
	if (!(arg->flags & DRM_CAPTURE_GRANT_CREATE_ADMIN) &&
	    !drm_is_current_master(file))
		return -EACCES;
	descriptors.capture_fd = get_unused_fd_flags(O_CLOEXEC);
	if (descriptors.capture_fd < 0)
		return descriptors.capture_fd;
	descriptors.control_fd = get_unused_fd_flags(O_CLOEXEC);
	if (descriptors.control_fd < 0) {
		ret = descriptors.control_fd;
		goto put_capture_fd;
	}
	ret = drm_capture_create_file_grant(dev, file, &target, arg->flags, &files);
	if (ret)
		goto put_control_fd;
	if (copy_to_user(u64_to_user_ptr(arg->files), &descriptors, sizeof(descriptors))) {
		ret = -EFAULT;
		goto put_files;
	}
	/* Input-only ioctl dispatch has no later copyout. Both transfers are infallible. */
	fd_install(descriptors.capture_fd, files.capture);
	fd_install(descriptors.control_fd, files.control);
	return 0;

put_files:
	drm_capture_files_put(&files);
put_control_fd:
	put_unused_fd(descriptors.control_fd);
put_capture_fd:
	put_unused_fd(descriptors.capture_fd);
	return ret;
}
