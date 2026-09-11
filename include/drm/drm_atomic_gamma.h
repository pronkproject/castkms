/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_ATOMIC_GAMMA_H__
#define __DRM_ATOMIC_GAMMA_H__

struct drm_atomic_commit;
struct drm_crtc;
struct drm_property_blob;
struct drm_prepare_owner;

int drm_atomic_set_legacy_gamma(struct drm_atomic_commit *state,
				struct drm_crtc *crtc, struct drm_property_blob *table);

/* Publish the retained legacy table under the controller's modeset lock. */
void drm_atomic_install_legacy_gamma(struct drm_atomic_commit *state);

int drm_atomic_commit_legacy_gamma(struct drm_crtc *crtc, struct drm_property_blob *table,
				   struct drm_prepare_owner *owner,
				   int (*validate)(struct drm_crtc *crtc, void *data), void *data);

#endif
