/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_ATOMIC_CONSTRAINTS_H__
#define __DRM_ATOMIC_CONSTRAINTS_H__

struct drm_atomic_commit;
struct drm_crtc_state;
struct drm_constraints_entry;

/*
 * Set a candidate binding in mutable atomic state under the CRTC modeset lock.
 * This retains entry, not availability or authority; real acceptance rechecks.
 * Omission retains duplicated state. A NULL entry is invalid, not a default.
 */
int drm_atomic_set_constraints_for_crtc(struct drm_crtc_state *state,
				       struct drm_constraints_entry *entry);

/*
 * Common atomic validation. Prepare adds the complete affected plane/color
 * state before driver checks and marks changed constraints as a modeset.
 * Check validates allocation limits and calls the provider's full-scene check.
 * The prototype admits only one independent output per transaction and no
 * asynchronous plane update when constraints are involved.
 * Fully disabling the CRTC with every plane detached retains its binding and
 * remains possible after catalog closure or backend failure. Such quiescence
 * selects no new entry and does not complete outstanding native source reads.
 */
int drm_atomic_constraints_prepare(struct drm_atomic_commit *state);
int drm_atomic_constraints_check(struct drm_atomic_commit *state);

/*
 * Final post-wait installation under the catalog lock. The caller holds modeset
 * and provider authority locks, and has completed all resource preparation.
 * The continuation must install state without failure; returning from it is
 * the acceptance boundary. The transaction already owns its backend binding.
 * No continuation is retained and no userspace acknowledgment is involved.
 */
int drm_atomic_constraints_install(struct drm_atomic_commit *state,
				    void (*install)(struct drm_atomic_commit *state));

#endif
