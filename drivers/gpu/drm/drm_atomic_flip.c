// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/export.h>
#include <drm/drm_atomic.h>
#include <drm/drm_atomic_flip.h>
#include <drm/drm_atomic_uapi.h>
#include <drm/drm_crtc.h>
#include <drm/drm_framebuffer.h>

#include "drm_crtc_internal.h"

/**
 * drm_atomic_set_legacy_flip - replace an active primary image without modesetting
 * @state: unchecked attempt with an initialized modeset acquire context
 * @crtc: controller currently displaying the primary plane
 * @fb: caller-retained replacement framebuffer
 *
 * Retains the replacement in pending plane state and preserves the existing
 * geometry and other plane properties. The accepted controller must be active
 * and its primary plane must have an image. The new image must use the same
 * pixel format and contain the pending source rectangle. Modifier constraints
 * and all other driver requirements remain part of complete atomic checking.
 *
 * The attempt is restricted to changes that need no modeset. No event is
 * attached and no state is installed. The caller establishes authority and
 * preparation separately, and retains its framebuffer reference until return.
 * Rebuilding after a wait must use the same retained replacement but fresh
 * controller and plane state.
 *
 * Return: zero on success or a negative error. A locking -EDEADLK requires
 * clearing and rebuilding the entire attempt after backing off.
 */
int drm_atomic_set_legacy_flip(struct drm_atomic_commit *state, struct drm_crtc *crtc,
			       struct drm_framebuffer *fb)
{
	struct drm_crtc_state *crtc_state, *old_crtc;
	struct drm_plane_state *plane_state, *old_plane;
	int ret;

	if (!crtc || !fb || crtc->dev != state->dev || fb->dev != state->dev ||
	    !state->acquire_ctx || state->checked || !crtc->primary)
		return -EINVAL;
	crtc_state = drm_atomic_get_crtc_state(state, crtc);
	if (IS_ERR(crtc_state))
		return PTR_ERR(crtc_state);
	old_crtc = drm_atomic_get_old_crtc_state(state, crtc);
	if (!old_crtc->active || !crtc_state->active)
		return -EINVAL;
	plane_state = drm_atomic_get_plane_state(state, crtc->primary);
	if (IS_ERR(plane_state))
		return PTR_ERR(plane_state);
	old_plane = drm_atomic_get_old_plane_state(state, crtc->primary);
	if (!old_plane->fb)
		return -EBUSY;
	if (old_plane->crtc != crtc || old_plane->fb->format->format != fb->format->format)
		return -EINVAL;
	ret = drm_framebuffer_check_src_coords(plane_state->src_x, plane_state->src_y,
					       plane_state->src_w, plane_state->src_h, fb);
	if (ret)
		return ret;
	ret = drm_atomic_set_crtc_for_plane(plane_state, crtc);
	if (ret)
		return ret;
	drm_atomic_set_fb_for_plane(plane_state, fb);
	state->allow_modeset = false;
	return 0;
}
EXPORT_SYMBOL_GPL(drm_atomic_set_legacy_flip);
