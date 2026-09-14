/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef __DRM_CAPTURE_READINESS_H__
#define __DRM_CAPTURE_READINESS_H__

#include <linux/types.h>

struct drm_capture_readiness;
struct wait_queue_head;

/*
 * Independently retained notification of available capture results. This object
 * owns no request, storage, authority or provider callback. Providers serialize
 * updates with their result queue; readers must recheck actual results after
 * waking. A true observation neither acknowledges a result nor reserves it.
 * Creation may sleep and returns an owned, initially false object or ERR_PTR.
 * Get retains a live non-NULL object; put releases a reference and accepts NULL.
 */
struct drm_capture_readiness *drm_capture_readiness_create(void);
struct drm_capture_readiness *drm_capture_readiness_get(struct drm_capture_readiness *readiness);
void drm_capture_readiness_put(struct drm_capture_readiness *readiness);

/*
 * Publish the current availability of results. A true update wakes waiters after
 * publishing readiness. False clears the observation without consuming a result.
 * Retain the object throughout each operation. Neither operation may sleep.
 */
void drm_capture_readiness_update(struct drm_capture_readiness *readiness, bool ready);
bool drm_capture_readiness_has_results(struct drm_capture_readiness *readiness);

/*
 * Register a waiter before checking readiness to avoid losing a notification.
 * Retain the object until the waiter is removed. The returned queue is borrowed;
 * retaining its address alone does not prevent destruction.
 */
struct wait_queue_head *drm_capture_readiness_waitqueue(struct drm_capture_readiness *readiness);

#endif
