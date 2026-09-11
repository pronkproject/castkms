/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_ATOMIC_FLIP_H__
#define __DRM_ATOMIC_FLIP_H__

struct drm_atomic_commit;
struct drm_crtc;
struct drm_framebuffer;

int drm_atomic_set_legacy_flip(struct drm_atomic_commit *state, struct drm_crtc *crtc,
			       struct drm_framebuffer *fb);

#endif
