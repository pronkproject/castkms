// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/export.h>
#include <drm/drm_atomic.h>
#include <drm/drm_atomic_uapi.h>
#include <drm/drm_crtc.h>

/**
 * drm_atomic_set_legacy_cursor_position - remember a position upon acceptance
 * @state: uncommitted update with an initialized modeset acquire context
 * @crtc: controller whose legacy cursor position will change
 * @x: requested signed x coordinate
 * @y: requested signed y coordinate
 *
 * Adds the controller to the update and records its requested legacy cursor
 * position without changing the accepted position or plane geometry. Repeated
 * assignments replace the pending coordinates. Clearing the update discards the
 * assignment. The installation path must publish the coordinates while the
 * controller's modeset lock is still held.
 *
 * The caller must set any cursor plane geometry separately and run ordinary
 * atomic validation. The remembered position also matters while the cursor is
 * invisible, so it is not derived from the visible plane's rectangle.
 *
 * Return: zero on success or a negative error. A locking -EDEADLK requires
 * clearing the update and backing off before rebuilding it.
 */
int drm_atomic_set_legacy_cursor_position(struct drm_atomic_commit *state,
					 struct drm_crtc *crtc, s32 x, s32 y)
{
	struct drm_crtc_state *crtc_state;
	struct __drm_crtcs_state *entry;

	if (state->dev != crtc->dev || !state->acquire_ctx || state->checked)
		return -EINVAL;
	crtc_state = drm_atomic_get_crtc_state(state, crtc);
	if (IS_ERR(crtc_state))
		return PTR_ERR(crtc_state);
	entry = &state->crtcs[drm_crtc_index(crtc)];
	entry->cursor_x = x;
	entry->cursor_y = y;
	entry->update_cursor_position = true;
	return 0;
}
EXPORT_SYMBOL_GPL(drm_atomic_set_legacy_cursor_position);
