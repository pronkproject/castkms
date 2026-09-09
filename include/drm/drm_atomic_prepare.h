/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_ATOMIC_PREPARE_H__
#define __DRM_ATOMIC_PREPARE_H__

#include <linux/types.h>

struct dma_fence;
struct drm_prepare_source;
struct drm_prepare_read_claim;
struct drm_prepare_admission_hold;
struct drm_prepare_domain;
struct drm_prepare_retirement_set;
struct drm_prepare_retirement_guard;

/* Related sources share a provider-owned admission domain, not a global lock.
 * Domain operations may sleep. Each source independently retains its domain.
 */
struct drm_prepare_domain *drm_prepare_domain_create(void);
struct drm_prepare_domain *drm_prepare_domain_get(struct drm_prepare_domain *domain);
void drm_prepare_domain_put(struct drm_prepare_domain *domain);
struct drm_prepare_source *
drm_prepare_source_create_in(struct drm_prepare_domain *domain, unsigned int capacity);

/*
 * Atomically hold admission for a fixed collection of sources in one domain.
 * Duplicate sources are coalesced. Empty sets are allowed; NULL members are
 * invalid and mixed domains return -EXDEV. The caller retains the input array
 * and every source throughout creation. Failure leaves no admission holds.
 * Final put releases the complete set without canceling existing readers.
 * All operations may sleep. The set neither grants access nor owns pixels.
 */
struct drm_prepare_retirement_set *
drm_prepare_retirement_set_create(struct drm_prepare_source * const *sources,
				  unsigned int count);
struct drm_prepare_retirement_set *
drm_prepare_retirement_set_get(struct drm_prepare_retirement_set *set);
void drm_prepare_retirement_set_put(struct drm_prepare_retirement_set *set);
/*
 * Zero means every member is ready, not that native readers have completed.
 * Terminal member failure takes precedence over other pending claims.
 */
int drm_prepare_retirement_set_ready(struct drm_prepare_retirement_set *set);
/*
 * Interruptibly wait for every admitted claim to be relinquished, not for native
 * reader completion. A failed member ends the wait with -EIO; signals return
 * -ERESTARTSYS. The borrowed set retains admission throughout. Do not hold locks
 * needed by claim owners, including display/provider locks, across the wait.
 * Ticket cancellation is separate and does not cancel a wait on an owned set.
 */
int drm_prepare_retirement_set_wait(struct drm_prepare_retirement_set *set);
/* On success, *fence owns native completion or is NULL; errors leave it untouched. */
int drm_prepare_retirement_set_completion(struct drm_prepare_retirement_set *set,
					struct dma_fence **fence);

/*
 * Preassemble owned admission and native completion before a commit decision.
 * Creation borrows a live set and requires readiness. Failure does not consume
 * the caller's set. The unique guard owns its set and completion independently;
 * destroy consumes it without waiting for or canceling native readers.
 * Completion is borrowed for the guard's lifetime and needs no allocation.
 * All operations may sleep. No display scope or acceptance is established here.
 */
struct drm_prepare_retirement_guard *
drm_prepare_retirement_guard_create(struct drm_prepare_retirement_set *set);
void drm_prepare_retirement_guard_destroy(struct drm_prepare_retirement_guard *guard);
struct dma_fence *
drm_prepare_retirement_guard_completion(struct drm_prepare_retirement_guard *guard);

/*
 * Kernel-only source-generation accounting, not an atomic ticket or pixel grant.
 * The provider establishes source authority and independently retains storage.
 * All operations may sleep and require a live reference. Claims retain their
 * source until release or abandonment consumes them. Permanent closure is
 * irreversible; temporary hold ownership is provided separately below.
 * Atomic transaction scope and ticket validation are separate responsibilities.
 * Capacity counts unresolved claims plus submitted reads still pending.
 */
/* Convenience constructor with a private domain for an independent source. */
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
