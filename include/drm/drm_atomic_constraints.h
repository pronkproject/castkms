/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_ATOMIC_CONSTRAINTS_H__
#define __DRM_ATOMIC_CONSTRAINTS_H__

struct drm_atomic_commit;
struct drm_crtc_state;
struct drm_constraints_entry;

/*
 * Set a candidate binding in a transaction's owned, unchecked proposed CRTC
 * state under its modeset lock. Live, detached and retiring states are invalid.
 * This retains entry, not availability or authority; real acceptance rechecks.
 * Omission retains duplicated state. A NULL entry is invalid, not a default.
 */
int drm_atomic_set_constraints_for_crtc(struct drm_crtc_state *state,
				       struct drm_constraints_entry *entry);

/*
 * Common atomic validation. Prepare adds the complete affected plane/color
 * state before driver checks and marks changed constraints as a modeset.
 * Check validates allocation limits and scalar property rules from proposed
 * state, then calls the provider's full-scene check.
 * Selecting or updating an enabled scene admits only one independent output
 * per transaction. Asynchronous plane updates are not supported.
 * Fully disabling the CRTC with every plane detached retains its binding and
 * remains possible after list closure or backend failure. Such quiescence
 * selects no new entry and does not complete outstanding native source reads.
 * Multiple CRTCs may be disabled together only when every CRTC in the
 * transaction is disabled, plane-free and retains its accepted binding.
 */
int drm_atomic_constraints_prepare(struct drm_atomic_commit *state);
int drm_atomic_constraints_check(struct drm_atomic_commit *state);

/*
 * Final post-wait installation under the list lock. The caller holds modeset
 * and provider authority locks, and has completed all resource preparation.
 * The continuation must install state without failure; returning from it is
 * the acceptance boundary. The transaction already owns its backend binding.
 * No continuation is retained and no userspace acknowledgment is involved.
 * Full multi-CRTC quiescence checks each binding under its list lock, then
 * installs once under the caller's modeset locks without nested list locks.
 * That path neither changes selection nor requires backend availability.
 */
int drm_atomic_constraints_install(struct drm_atomic_commit *state,
				    void (*install)(struct drm_atomic_commit *state));

#endif
