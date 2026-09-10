/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_ATOMIC_PREPARE_INTERNAL_H__
#define __DRM_ATOMIC_PREPARE_INTERNAL_H__

#include <linux/wait.h>

struct drm_prepare_source;
struct drm_prepare_admission_hold;
struct drm_prepare_retirement_set;
struct drm_prepare_domain;

/* The callback runs in TASK_RUNNING and may sleep while checking readiness. */
int drm_prepare_wait_until_ready(wait_queue_head_t *queue, int (*ready)(void *data), void *data);

/* Immutable domain identity, borrowed for the source reference's lifetime. */
struct drm_prepare_domain *drm_prepare_source_domain(struct drm_prepare_source *source);

/* Borrowed notification ownership, independent of admission holds. */
struct drm_prepare_domain *
drm_prepare_admission_hold_domain(struct drm_prepare_admission_hold *hold);
wait_queue_head_t *drm_prepare_domain_waitqueue(struct drm_prepare_domain *domain);
struct drm_prepare_domain *
drm_prepare_retirement_set_domain(struct drm_prepare_retirement_set *set);

/* Borrowed for the set's lifetime; an empty set has no queue and is already ready. */
wait_queue_head_t *
drm_prepare_retirement_set_waitqueue(struct drm_prepare_retirement_set *set);

/* Sources are distinct, live and stable for the call. Output is private until success. */
int drm_prepare_hold_sources(struct drm_prepare_source * const *sources,
			     struct drm_prepare_admission_hold **holds,
			     unsigned int count);

#endif
