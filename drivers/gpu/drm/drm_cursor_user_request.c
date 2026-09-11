// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <drm/drm_atomic_prepare_auth.h>
#include <drm/drm_atomic_prepare_owner.h>
#include <drm/drm_auth.h>
#include <drm/drm_crtc.h>
#include <drm/drm_file.h>
#include <drm/drm_fourcc.h>
#include <drm/drm_framebuffer.h>
#include <drm/drm_lease.h>

#include "drm_crtc_internal.h"

static int validate_cursor_update(const struct drm_cursor_update *update, void *data)
{
	struct drm_file *file = data;

	if (!drm_is_current_master(file) ||
	    !drm_lease_held(file, update->crtc->base.id) ||
	    !drm_lease_held(file, update->crtc->cursor->base.id))
		return -EACCES;
	return 0;
}

int drm_mode_cursor_with_preparation(struct drm_crtc *crtc,
				     const struct drm_mode_cursor2 *args,
				     struct drm_file *file)
{
	struct drm_cursor_update update = {
		.crtc = crtc,
		.update_image = args->flags & DRM_MODE_CURSOR_BO,
		.update_position = args->flags & DRM_MODE_CURSOR_MOVE,
		.x = args->x, .y = args->y,
		.hot_x = args->hot_x, .hot_y = args->hot_y,
	};
	struct drm_prepare_owner *owner;
	int ret;

	if (!crtc->cursor || !crtc->funcs->cursor_request)
		return -EOPNOTSUPP;
	owner = drm_file_prepare_owner(file);
	if (IS_ERR(owner))
		return PTR_ERR(owner);
	ret = validate_cursor_update(&update, file);
	if (ret)
		goto out;
	if (update.update_image && args->handle) {
		struct drm_mode_fb_cmd2 fb = {
			.width = args->width, .height = args->height,
			.pixel_format = DRM_FORMAT_ARGB8888,
			.handles = { args->handle },
		};

		if (args->width > U16_MAX || args->height > U16_MAX) {
			ret = -ERANGE;
			goto out;
		}
		fb.pitches[0] = args->width * 4;
		update.fb = drm_internal_framebuffer_create(crtc->dev, &fb, file);
		if (IS_ERR(update.fb)) {
			ret = PTR_ERR(update.fb);
			update.fb = NULL;
			goto out;
		}
	}
	ret = crtc->funcs->cursor_request(&update, owner, validate_cursor_update, file);
out:
	if (update.fb)
		drm_framebuffer_put(update.fb);
	drm_prepare_owner_put(owner);
	return ret;
}
