/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_ATOMIC_PREPARE_SCOPE_H__
#define __DRM_ATOMIC_PREPARE_SCOPE_H__

#include <linux/types.h>

struct drm_prepare_source;
struct drm_prepare_retirement_set;
struct drm_prepare_scope;

/* KMS represents CRTC membership with a 32-bit mask. */
#define DRM_PREPARE_SCOPE_MAX_OUTPUTS 32

struct drm_prepare_scope_entry {
	u32 crtc_id;
	struct drm_prepare_source *source;
};

/*
 * Copy an immutable cohort of output generations. Nonzero CRTC IDs must be
 * unique; each source is a live, non-NULL generation object. The provider must
 * create a new source for each accepted use, including same-framebuffer updates
 * and transitions to a blank output. Retaining sources prevents identity reuse.
 * Empty cohorts are valid. All sources must belong to one admission domain.
 * Counts above DRM_PREPARE_SCOPE_MAX_OUTPUTS return -E2BIG.
 *
 * The caller stabilizes the complete cohort with its display locks and retains
 * input sources throughout construction. The scope owns source references, not
 * pixels, display objects, admission holds, or authority. Device and caller
 * identity must be validated separately. All operations may sleep.
 */
struct drm_prepare_scope *
drm_prepare_scope_create(const struct drm_prepare_scope_entry *entries,
			 unsigned int count);
void drm_prepare_scope_destroy(struct drm_prepare_scope *scope);

/*
 * Compare the complete observed cohort, irrespective of entry order. Malformed
 * input returns -EINVAL; oversized lists return -E2BIG. Changed membership or
 * generation returns -ESTALE.
 * The caller retains observed sources and holds display locks across validation
 * and the subsequent acceptance decision. Success alone does not grant access.
 */
int drm_prepare_scope_validate(const struct drm_prepare_scope *scope,
			       const struct drm_prepare_scope_entry *observed,
			       unsigned int count);

/*
 * Hold admission for exactly the captured sources. The result owns its holds
 * independently of scope lifetime. Validate observed display scope before using
 * those holds for acceptance. Creation never expands the captured cohort.
 */
struct drm_prepare_retirement_set *
drm_prepare_scope_hold(const struct drm_prepare_scope *scope);

#endif
