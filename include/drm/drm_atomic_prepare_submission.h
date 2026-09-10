/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_ATOMIC_PREPARE_SUBMISSION_H__
#define __DRM_ATOMIC_PREPARE_SUBMISSION_H__

struct drm_atomic_commit;
struct drm_crtc;
struct drm_prepare_owner;
struct drm_prepare_ticket;

/*
 * Assemble an issuer-bound submission independently of descriptor transport.
 * The caller obtains the issuer before taking display locks. Each selected
 * CRTC must belong to the transaction's device and have a state in it. All
 * selected CRTCs must use the same ticket. References are retained, not files.
 */
int drm_atomic_prepare_submission_init(struct drm_atomic_commit *state,
				       struct drm_prepare_owner *owner);
int drm_atomic_prepare_submission_set(struct drm_atomic_commit *state,
				      struct drm_crtc *crtc,
				      struct drm_prepare_ticket *ticket);
int drm_atomic_prepare_submission_attach(struct drm_atomic_commit *state);
void drm_atomic_prepare_submission_clear(struct drm_atomic_commit *state);

#endif
