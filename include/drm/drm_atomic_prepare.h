/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_ATOMIC_PREPARE_H__
#define __DRM_ATOMIC_PREPARE_H__

#include <linux/types.h>

struct dma_fence;
struct drm_prepare_source;
struct drm_prepare_read_claim;
struct drm_prepare_admission_hold;

/*
 * Kernel-only source-generation accounting, not an atomic ticket or pixel grant.
 * The provider establishes source authority and independently retains storage.
 * All operations may sleep and require a live reference. Claims retain their
 * source until release or abandonment consumes them. Permanent closure is
 * irreversible; temporary hold ownership is provided separately below.
 * Atomic ticket/cohort ownership is not implemented by this primitive.
 * Capacity counts unresolved claims plus submitted reads still pending.
 */
struct drm_prepare_source *drm_prepare_source_create(unsigned int capacity);
struct drm_prepare_source *drm_prepare_source_get(struct drm_prepare_source *source);
void drm_prepare_source_put(struct drm_prepare_source *source);
struct drm_prepare_read_claim *drm_prepare_source_claim(struct drm_prepare_source *source);
void drm_prepare_source_seal(struct drm_prepare_source *source);
/* Zero means permanently sealed with no unresolved claims, not GPU completion. */
int drm_prepare_source_ready(struct drm_prepare_source *source);

/*
 * A temporary admission hold retains its source. Its final put allows new
 * claims only if no other hold or permanent closure remains. Keep an owned
 * hold reference through preparation and transfer; readiness alone is not an
 * owned hold. Released native reads remain accounted for after reopening.
 * Source-level ready/completion still require permanent closure; temporary
 * owners use the hold-specific operations below.
 */
struct drm_prepare_admission_hold *drm_prepare_source_hold_admission(struct drm_prepare_source *source);
struct drm_prepare_admission_hold *
drm_prepare_admission_hold_get(struct drm_prepare_admission_hold *hold);
void drm_prepare_admission_hold_put(struct drm_prepare_admission_hold *hold);
int drm_prepare_admission_hold_ready(struct drm_prepare_admission_hold *hold);
int drm_prepare_admission_hold_completion(struct drm_prepare_admission_hold *hold,
					struct dma_fence **fence);

/*
 * Release consumes a claim. NULL means no access or completed synchronous CPU
 * access; otherwise the borrowed native fence must cover all submitted reads.
 * The trusted provider promises no further access under the claim. The fence
 * must not depend on future userspace submission or downstream destination use.
 * An abandoned claim poisons preparation; it never invents successful release.
 */
void drm_prepare_read_release(struct drm_prepare_read_claim *read, struct dma_fence *fence);
void drm_prepare_read_abandon(struct drm_prepare_read_claim *read);

/*
 * Only a ready source yields its fixed native completion set. On success *fence
 * owns a native reference, or is NULL if no wait is needed. Output is untouched
 * on error. Native errors still end access; they do not establish valid pixels.
 * No framebuffer, KMS commit, capture authority or destination is owned here.
 */
int drm_prepare_source_completion(struct drm_prepare_source *source,
				  struct dma_fence **fence);

#endif
