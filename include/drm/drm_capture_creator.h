/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_CAPTURE_CREATOR_H_
#define __DRM_CAPTURE_CREATOR_H_

#include <linux/types.h>

struct drm_capture_authority;
struct drm_capture_creator;
struct drm_capture_registration;

/*
 * A unique owner of grants to revoke at close. The limit is provider policy.
 * All operations may sleep. The caller retains the creator across registration
 * and excludes registration racing close. Close consumes the creator reference;
 * registrations retain tracking storage, not the lifetime that permits grants.
 */
struct drm_capture_creator *drm_capture_creator_create(u32 limit);
void drm_capture_creator_close(struct drm_capture_creator *creator);

/*
 * Register already-authorized authority. No pixel permission is conferred.
 * Success transfers one registration, independently removable after close.
 * Removal does not revoke. Close revokes all registrations still tracked at
 * its linearization point, outside the registry lock. Neither operation may
 * run while holding locks needed by authority cleanup or final policy release.
 */
struct drm_capture_registration *
drm_capture_creator_register(struct drm_capture_creator *creator,
			     struct drm_capture_authority *authority);
void drm_capture_registration_remove(struct drm_capture_registration *registration);

#endif
