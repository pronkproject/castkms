/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_CONSTRAINTS_OWNER_H__
#define __DRM_CONSTRAINTS_OWNER_H__

struct drm_device;

/* Internal mode-configuration lifetime; initialize before registration. */
int drm_constraints_owner_init(struct drm_device *dev);
void drm_constraints_owner_stop(struct drm_device *dev);
void drm_constraints_owner_fini(struct drm_device *dev);

/*
 * Call with master_mutex held. Loss follows source-authority revocation and
 * precedes releasing that mutex. Replacement ownership must check readiness
 * under the same mutex before publishing the new master. The worker performs
 * prepared recovery without holding master_mutex; queued work retains dev.
 *
 * Check returns zero for ready/nonparticipating devices, EBUSY while pending,
 * the recovery error after failure, or ENODEV after shutdown. Retry schedules
 * failed recovery only; it does not authorize a replacement or reopen lists.
 */
void drm_constraints_owner_lost(struct drm_device *dev);
int drm_constraints_owner_check(struct drm_device *dev);
void drm_constraints_owner_retry(struct drm_device *dev);

/* Join current recovery without master/modeset locks; grants no authority. */
void drm_constraints_owner_flush(struct drm_device *dev);

#endif
