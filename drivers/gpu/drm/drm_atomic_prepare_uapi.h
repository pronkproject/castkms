/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_ATOMIC_PREPARE_UAPI_H__
#define __DRM_ATOMIC_PREPARE_UAPI_H__

#include <linux/types.h>

struct drm_device;
struct drm_file;
struct drm_atomic_commit;
struct drm_crtc;

int drm_mode_prepare_replace_ioctl(struct drm_device *dev, void *data,
				   struct drm_file *file);

int drm_atomic_prepare_set_fd(struct drm_atomic_commit *state,
			     struct drm_crtc *crtc, u64 value);

#endif
