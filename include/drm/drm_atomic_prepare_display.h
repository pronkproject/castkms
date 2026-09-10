/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_ATOMIC_PREPARE_DISPLAY_H__
#define __DRM_ATOMIC_PREPARE_DISPLAY_H__

struct drm_device;
struct drm_crtc;
struct drm_atomic_commit;
struct drm_prepare_source;
struct drm_prepare_output_generation;
struct drm_prepare_owner;
struct drm_prepare_ticket;

/*
 * Enable output-generation accounting during unregistered device setup.
 * The driver must use the atomic CRTC state duplication/destruction helpers
 * and the shared atomic installation path. Capacity limits admitted reads per
 * generation, not media queue depth. Enabling accounting grants no source access.
 */
int drm_atomic_prepare_display_init(struct drm_device *dev, unsigned int capacity);
void drm_atomic_prepare_display_fini(struct drm_device *dev);

/*
 * Return a borrowed accounting generation for the currently accepted output.
 * The caller holds the CRTC modeset lock and retains it across use/reference
 * acquisition. The first observation allocates the initial (possibly blank)
 * generation. No framebuffer, pixel authority or source-read lease is returned.
 */
struct drm_prepare_source *drm_atomic_prepare_crtc_source(struct drm_crtc *crtc);

/*
 * Capture exactly the borrowed CRTC list under its held modeset locks. The
 * caller authorizes the complete list; a NULL owner creates a kernel ticket.
 * CRTCs must belong to one participating device. No descriptor is installed.
 */
struct drm_prepare_ticket *
drm_atomic_prepare_crtcs(struct drm_crtc * const *crtcs, unsigned int count,
			 struct drm_prepare_owner *owner);

/* Atomic core: allocate distinct generations for checked replacement states. */
int drm_atomic_prepare_display_check(struct drm_atomic_commit *state);

/* Observe all old CRTC generations in the expanded, locked transaction. */
int drm_atomic_prepare_display_observe(struct drm_atomic_commit *state,
				       struct drm_prepare_output_generation *entries,
				       unsigned int capacity);

#endif
