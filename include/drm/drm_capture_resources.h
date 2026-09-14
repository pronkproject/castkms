/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_CAPTURE_RESOURCES_H_
#define __DRM_CAPTURE_RESOURCES_H_

#include <linux/types.h>

struct drm_capture_resources;

/*
 * Fixed-capacity names for a client's resources. The provider owns one payload
 * per slot and serializes every operation, including check through insert.
 * Names are nonzero and increase on successful insertion only. Removing an
 * entry frees its slot but never makes its name reusable. No authority, pixel
 * storage or provider cleanup is owned by this table. Destroy after removing
 * entries or arranging independent cleanup of all remaining payloads.
 */
struct drm_capture_resources *drm_capture_resources_create(u32 limit);
void drm_capture_resources_destroy(struct drm_capture_resources *resources);
/* Return a slot index or negative errno. Check does not reserve or consume it. */
int drm_capture_resources_check(const struct drm_capture_resources *resources, u64 id);
int drm_capture_resources_insert(struct drm_capture_resources *resources, u64 id);
int drm_capture_resources_find(const struct drm_capture_resources *resources, u64 id);
int drm_capture_resources_remove(struct drm_capture_resources *resources, u64 id);

#endif
