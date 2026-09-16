/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_CONSTRAINTS_OUTPUT_H__
#define __DRM_CONSTRAINTS_OUTPUT_H__

struct drm_atomic_commit;
struct drm_crtc;
struct drm_crtc_state;
struct drm_constraints_list;
struct drm_constraints_entry;

/**
 * struct drm_constraints_output_ops - complete provider validation
 * @check: validate the complete proposed scene against the retained backend
 *
 * Check is called with modeset locks and the list lock held, both during
 * validation and before acceptance. It must not modify the transaction or its
 * proposed object states, change external state, acquire modeset locks, reenter
 * list operations or wait for userspace. All resources required to accept the
 * entry must be ready before offering it. Return zero or a negative errno.
 * The entry exposes its immutable identity, description and retained provider
 * context. The table must remain valid throughout the CRTC lifetime.
 */
struct drm_constraints_output_ops {
	int (*check)(const struct drm_atomic_commit *state,
		     const struct drm_crtc_state *crtc_state,
		     const struct drm_constraints_entry *entry);
};

/*
 * Attach a ready default before device registration, with no enabled state or
 * pending commits. Validate object/property membership and rules within existing
 * allocation/property support without rewriting discovery. Allocation admits
 * target buffers before selection. Only atomic drivers using common state and
 * installation helpers may opt in. Initialization owns references only on success.
 */
int drm_constraints_crtc_init(struct drm_crtc *crtc, struct drm_constraints_entry *initial,
			      unsigned int limit, const struct drm_constraints_output_ops *ops);
void drm_constraints_crtc_fini(struct drm_crtc *crtc);
/* Borrowed list, or NULL; valid throughout the CRTC lifetime. */
struct drm_constraints_list *drm_constraints_crtc_list(struct drm_crtc *crtc);

/*
 * Borrow the fixed default retained at attachment, or NULL for an unattached
 * output. It survives offer withdrawal and selection changes. The caller
 * retains the CRTC lifetime; the reference grants no readiness or authority.
 * State reset still preserves accepted selection rather than restoring this
 * default. Restoration requires a separate checked atomic transaction.
 */
struct drm_constraints_entry *drm_constraints_crtc_default(struct drm_crtc *crtc);

/* Publish a ready entry after validating device, CRTC and existing plane scope. */
int drm_constraints_crtc_add(struct drm_crtc *crtc, struct drm_constraints_entry *entry);

/* Atomic state helper: initialize the owned binding without allocating. */
void drm_constraints_crtc_state_init(struct drm_crtc_state *state);

#endif
