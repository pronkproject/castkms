/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_CAPTURE_UAPI_H__
#define __DRM_CAPTURE_UAPI_H__

struct drm_device;
struct drm_file;
struct file;

long drm_capture_client_ioctl(struct file *file, unsigned int cmd, unsigned long arg);

int drm_mode_create_capture_grant_ioctl(struct drm_device *dev, void *data,
					struct drm_file *file);

#endif
