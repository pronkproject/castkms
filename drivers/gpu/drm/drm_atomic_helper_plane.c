// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/export.h>
#include <drm/drm_atomic.h>
#include <drm/drm_atomic_helper.h>
#include <drm/drm_atomic_prepare_request.h>
#include <drm/drm_atomic_uapi.h>
#include <drm/drm_framebuffer.h>
#include <drm/drm_plane.h>

#include "drm_crtc_internal.h"

struct plane_request {
	const struct drm_plane_update *update;
	int (*validate)(const struct drm_plane_update *update, void *data);
	void *data;
};

static int build_plane_update(struct drm_atomic_commit *state, void *data)
{
	struct plane_request *request = data;
	const struct drm_plane_update *update = request->update;
	struct drm_plane_state *plane_state;
	int ret;

	ret = drm_modeset_lock_all_ctx(state->dev, state->acquire_ctx);
	if (ret)
		return ret;
	if (request->validate) {
		ret = request->validate(update, request->data);
		if (ret)
			return ret;
	}
	plane_state = drm_atomic_get_plane_state(state, update->plane);
	if (IS_ERR(plane_state))
		return PTR_ERR(plane_state);
	if (!update->fb) {
		if (plane_state->crtc && plane_state->crtc->cursor == update->plane)
			state->legacy_cursor_update = true;
		return __drm_atomic_helper_disable_plane(update->plane, plane_state);
	}
	ret = drm_atomic_set_crtc_for_plane(plane_state, update->crtc);
	if (ret)
		return ret;
	drm_atomic_set_fb_for_plane(plane_state, update->fb);
	plane_state->crtc_x = update->crtc_x;
	plane_state->crtc_y = update->crtc_y;
	plane_state->crtc_w = update->crtc_w;
	plane_state->crtc_h = update->crtc_h;
	plane_state->src_x = update->src_x;
	plane_state->src_y = update->src_y;
	plane_state->src_w = update->src_w;
	plane_state->src_h = update->src_h;
	if (update->crtc->cursor == update->plane)
		state->legacy_cursor_update = true;
	return 0;
}

/**
 * drm_atomic_helper_update_plane_request - rebuild a resolved legacy plane update
 * @update: stable description with caller-retained objects and framebuffer
 * @owner: retained issuer, revoked when the caller's authority ends
 * @validate: optional selected-object authorization check on every attempt
 * @data: caller-owned validation data
 *
 * The caller holds no modeset locks and keeps all inputs and the device alive
 * until return. Validation runs under all modeset locks before each attempt.
 * Issuer revocation excludes installation after validation; any authority not
 * represented by the issuer remains the caller's responsibility.
 *
 * A NULL framebuffer disables the plane and clears its geometry. Otherwise
 * only the selected controller, framebuffer and rectangles replace current
 * state. The complete update passes ordinary atomic checks before preparation
 * and blocking acceptance. The helper does not call custom legacy callbacks.
 *
 * Returns: zero on success or a negative error.
 */
int drm_atomic_helper_update_plane_request(const struct drm_plane_update *update,
					   struct drm_prepare_owner *owner,
					   int (*validate)(const struct drm_plane_update *update,
							   void *data),
					   void *data)
{
	struct plane_request request = { update, validate, data };
	struct drm_device *dev;

	if (!update || !update->plane || !!update->fb != !!update->crtc)
		return -EINVAL;
	dev = update->plane->dev;
	if (update->fb && (update->fb->dev != dev || update->crtc->dev != dev))
		return -EINVAL;
	return drm_atomic_commit_request_owned(dev, owner, build_plane_update, &request);
}
EXPORT_SYMBOL_GPL(drm_atomic_helper_update_plane_request);
