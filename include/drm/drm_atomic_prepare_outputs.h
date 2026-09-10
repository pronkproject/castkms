/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_ATOMIC_PREPARE_OUTPUTS_H__
#define __DRM_ATOMIC_PREPARE_OUTPUTS_H__

#include <linux/types.h>

struct drm_prepare_source;
struct drm_prepare_retirement_set;
struct drm_prepare_outputs;

/* KMS represents CRTC membership with a 32-bit mask. */
#define DRM_PREPARE_MAX_OUTPUTS 32

struct drm_prepare_output_generation {
	u32 crtc_id;
	struct drm_prepare_source *source;
};

/*
 * Copy an immutable list of output generations. Nonzero CRTC IDs must be
 * unique; each source is a live, non-NULL generation object. The provider must
 * create a new source for each accepted use, including same-framebuffer updates
 * and transitions to a blank output. Retaining sources prevents identity reuse.
 * An empty list is valid. All sources must belong to one admission domain.
 * Lists exceeding DRM_PREPARE_MAX_OUTPUTS return -E2BIG.
 *
 * The caller stabilizes the complete list with its display locks and retains
 * input sources throughout construction. The list owns source references, not
 * pixels, display objects, admission holds, or authority. Device and caller
 * identity must be validated separately. All operations may sleep.
 */
struct drm_prepare_outputs *
drm_prepare_outputs_create(const struct drm_prepare_output_generation *entries,
			 unsigned int count);
void drm_prepare_outputs_destroy(struct drm_prepare_outputs *outputs);

/*
 * Compare the complete observed list, irrespective of entry order. Malformed
 * input returns -EINVAL, oversized lists return -E2BIG, and changed membership
 * or generation returns -ESTALE.
 * The caller retains observed sources and holds display locks across validation
 * and the subsequent acceptance decision. Success alone does not grant access.
 */
int drm_prepare_outputs_validate(const struct drm_prepare_outputs *outputs,
			       const struct drm_prepare_output_generation *observed,
			       unsigned int count);

/*
 * Hold admission for exactly the captured sources. The result owns its holds
 * independently of the list's lifetime. Validate observed output generations
 * before using those holds for acceptance. Creation never expands the list.
 */
struct drm_prepare_retirement_set *
drm_prepare_outputs_hold(const struct drm_prepare_outputs *outputs);

#endif
