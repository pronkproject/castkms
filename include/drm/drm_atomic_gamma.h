/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_ATOMIC_GAMMA_H__
#define __DRM_ATOMIC_GAMMA_H__

struct drm_atomic_commit;
struct drm_crtc;
struct drm_property_blob;

int drm_atomic_set_legacy_gamma(struct drm_atomic_commit *state,
				struct drm_crtc *crtc, struct drm_property_blob *table);

/* Publish the retained legacy table under the controller's modeset lock. */
void drm_atomic_install_legacy_gamma(struct drm_atomic_commit *state);

#endif
