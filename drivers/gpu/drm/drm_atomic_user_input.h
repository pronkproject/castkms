/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_ATOMIC_USER_INPUT_H__
#define __DRM_ATOMIC_USER_INPUT_H__

#include <linux/types.h>

struct drm_mode_atomic;

/* Copied identifiers and values; resource references are resolved separately. */
struct drm_atomic_user_input {
	u32 object_count;
	u32 property_count;
	const u32 *objects;
	const u32 *counts;
	const u32 *properties;
	const u64 *values;
};

struct drm_atomic_user_input *drm_atomic_copy_user_input(const struct drm_mode_atomic *arg);
void drm_atomic_free_user_input(struct drm_atomic_user_input *input);

#endif
