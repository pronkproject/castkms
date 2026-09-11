/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_ATOMIC_REQUEST_INTERNAL_H__
#define __DRM_ATOMIC_REQUEST_INTERNAL_H__

#include <linux/types.h>

struct drm_mode_object;
struct drm_property;

bool drm_atomic_request_supports_property(struct drm_mode_object *object,
					 struct drm_property *property);

#endif
