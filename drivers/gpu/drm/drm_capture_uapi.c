// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/fcntl.h>
#include <linux/file.h>
#include <linux/uaccess.h>
#include <drm/drm_capture_grant.h>
#include <drm/drm_capture_file.h>
#include <uapi/drm/drm_capture.h>

#include "drm_capture_uapi.h"

long drm_capture_client_ioctl(struct file *file, unsigned int cmd, unsigned long arg)
{
	struct drm_capture_describe output = {};
	struct drm_capture_description description;
	int ret;

	if (cmd != DRM_IOCTL_CAPTURE_DESCRIBE)
		return -ENOTTY;
	ret = drm_capture_client_describe(file, &description);
	if (ret)
		return ret;
	output.id = description.id;
	output.width = description.width;
	output.height = description.height;
	output.format = description.format;
	output.max_requests = description.max_requests;
	output.modifier = description.modifier;
	if (copy_to_user((void __user *)arg, &output, sizeof(output)))
		return -EFAULT;
	return 0;
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

	if (!target.crtc_id || !target.connector_id || arg->flags ||
	    arg->reserved[0] || arg->reserved[1] || arg->reserved[2])
		return -EINVAL;
	descriptors.capture_fd = get_unused_fd_flags(O_CLOEXEC);
	if (descriptors.capture_fd < 0)
		return descriptors.capture_fd;
	descriptors.control_fd = get_unused_fd_flags(O_CLOEXEC);
	if (descriptors.control_fd < 0) {
		ret = descriptors.control_fd;
		goto put_capture_fd;
	}
	ret = drm_capture_create_file_grant(dev, file, &target, &files);
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
