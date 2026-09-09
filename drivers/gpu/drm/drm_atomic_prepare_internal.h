/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_ATOMIC_PREPARE_INTERNAL_H__
#define __DRM_ATOMIC_PREPARE_INTERNAL_H__

#include <linux/wait.h>

struct drm_prepare_source;
struct drm_prepare_admission_hold;
struct drm_prepare_retirement_set;

/* The callback runs in TASK_RUNNING and may sleep while checking readiness. */
int drm_prepare_wait_until_ready(wait_queue_head_t *queue, int (*ready)(void *data), void *data);

/* Borrowed for the hold's lifetime; all holds in one set retain the same domain. */
wait_queue_head_t *
drm_prepare_admission_hold_waitqueue(struct drm_prepare_admission_hold *hold);

/* Borrowed for the set's lifetime; an empty set has no queue and is already ready. */
wait_queue_head_t *
drm_prepare_retirement_set_waitqueue(struct drm_prepare_retirement_set *set);

/* Sources are distinct, live and stable for the call. Output is private until success. */
int drm_prepare_hold_sources(struct drm_prepare_source * const *sources,
			     struct drm_prepare_admission_hold **holds,
			     unsigned int count);

#endif
