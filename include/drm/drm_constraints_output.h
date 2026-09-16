/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_CONSTRAINTS_OUTPUT_H__
#define __DRM_CONSTRAINTS_OUTPUT_H__

struct drm_atomic_commit;
struct drm_crtc;
struct drm_crtc_state;
struct drm_constraints_catalog;
struct drm_constraints_entry;

/**
 * struct drm_constraints_output_ops - complete provider validation
 * @check: validate the complete proposed scene against the retained backend
 *
 * Check is called with modeset locks and the catalog lock held, both during
 * validation and before acceptance. It must not change external state, acquire
 * modeset locks, reenter catalog operations or wait for userspace. All resources
 * required to accept the entry must be ready before offering it. Return zero
 * or a negative errno. Backend is the entry's retained provider context.
 * The table must remain valid throughout the CRTC lifetime.
 */
struct drm_constraints_output_ops {
	int (*check)(struct drm_atomic_commit *state,
		     const struct drm_crtc_state *crtc_state, void *backend);
};

/*
 * Attach a ready default before device registration, with no enabled state or
 * pending commits. Validate existing plane membership but do not rewrite plane
 * discovery. The provider's allocation path must admit target buffers before
 * selection. Only atomic drivers using common state and installation helpers
 * may opt in. Initialization owns references only on success.
 */
int drm_constraints_crtc_init(struct drm_crtc *crtc, struct drm_constraints_entry *initial,
			      unsigned int limit, const struct drm_constraints_output_ops *ops);
void drm_constraints_crtc_fini(struct drm_crtc *crtc);
/* Borrowed catalog, or NULL; valid throughout the CRTC lifetime. */
struct drm_constraints_catalog *drm_constraints_crtc_catalog(struct drm_crtc *crtc);

/* Atomic state helper: initialize the owned binding without allocating. */
void drm_constraints_crtc_state_init(struct drm_crtc_state *state);

#endif
