/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_CONSTRAINTS_UAPI_H__
#define __DRM_CONSTRAINTS_UAPI_H__

struct drm_device;
struct drm_file;

int drm_mode_list_constraints_ioctl(struct drm_device *dev, void *data,
				    struct drm_file *file);

#endif
