// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/export.h>
#include <drm/drm_atomic.h>
#include <drm/drm_atomic_helper.h>
#include <drm/drm_atomic_prepare_request.h>
#include <drm/drm_atomic_uapi.h>
#include <drm/drm_framebuffer.h>

#include "drm_crtc_internal.h"

struct cursor_request {
	const struct drm_cursor_update *update;
	int (*validate)(const struct drm_cursor_update *update, void *data);
	void *data;
};

static int build_cursor_update(struct drm_atomic_commit *state, void *data)
{
	struct cursor_request *request = data;
	const struct drm_cursor_update *update = request->update;
	struct drm_crtc *crtc = update->crtc;
	struct drm_plane *plane = crtc->cursor;
	struct drm_plane_state *plane_state;
	struct drm_framebuffer *fb;
	int ret;

	ret = drm_modeset_lock_all_ctx(state->dev, state->acquire_ctx);
	if (ret)
		return ret;
	if (request->validate) {
		ret = request->validate(update, request->data);
		if (ret)
			return ret;
	}
	plane_state = drm_atomic_get_plane_state(state, plane);
	if (IS_ERR(plane_state))
		return PTR_ERR(plane_state);
	if (update->update_position) {
		ret = drm_atomic_set_legacy_cursor_position(state, crtc, update->x, update->y);
		if (ret)
			return ret;
	}
	state->legacy_cursor_update = true;
	fb = update->update_image ? update->fb : plane_state->fb;
	if (!fb)
		return __drm_atomic_helper_disable_plane(plane, plane_state);
	if (fb->width > U16_MAX || fb->height > U16_MAX)
		return -ERANGE;
	ret = drm_atomic_set_crtc_for_plane(plane_state, crtc);
	if (ret)
		return ret;
	if (update->update_image) {
		drm_atomic_set_fb_for_plane(plane_state, fb);
		if (plane->hotspot_x_property)
			plane_state->hotspot_x = update->hot_x;
		if (plane->hotspot_y_property)
			plane_state->hotspot_y = update->hot_y;
	}
	plane_state->crtc_x = update->update_position ? update->x : crtc->cursor_x;
	plane_state->crtc_y = update->update_position ? update->y : crtc->cursor_y;
	plane_state->crtc_w = fb->width;
	plane_state->crtc_h = fb->height;
	plane_state->src_x = plane_state->src_y = 0;
	plane_state->src_w = fb->width << 16;
	plane_state->src_h = fb->height << 16;
	return 0;
}

/**
 * drm_atomic_helper_cursor_request - rebuild a resolved universal cursor command
 * @update: immutable description with caller-retained image and controller
 * @owner: retained issuer whose revocation excludes installation
 * @validate: optional authorization callback for each attempt
 * @data: caller-owned validation data
 *
 * Call without modeset locks and retain every input until return. The helper
 * validates and prepares each rebuilt update under all modeset locks, dropping
 * attempted state and locks before waiting for readers. Unrequested image or
 * position state is read afresh after waiting. A rejected attempt publishes
 * neither its hotspot nor its remembered cursor position.
 *
 * The controller must have a universal cursor. Complete atomic checks precede
 * preparation and acceptance; custom legacy cursor callbacks are not invoked.
 * Any authorization not represented by @owner remains the caller's duty.
 *
 * Returns: zero on success or a negative error.
 */
int drm_atomic_helper_cursor_request(const struct drm_cursor_update *update,
				     struct drm_prepare_owner *owner,
				     int (*validate)(const struct drm_cursor_update *update,
						     void *data),
				     void *data)
{
	struct cursor_request request = { update, validate, data };
	struct drm_device *dev;

	if (!update || !update->crtc ||
	    (!update->update_image && !update->update_position) ||
	    (!update->update_image && update->fb))
		return -EINVAL;
	dev = update->crtc->dev;
	if (update->fb && update->fb->dev != dev)
		return -EINVAL;
	if (!update->crtc->cursor)
		return -EOPNOTSUPP;
	return drm_atomic_commit_request_owned(dev, owner, build_cursor_update, &request);
}
EXPORT_SYMBOL_GPL(drm_atomic_helper_cursor_request);
