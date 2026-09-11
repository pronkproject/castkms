// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <drm/drm_atomic_prepare_auth.h>
#include <drm/drm_atomic_prepare_owner.h>
#include <drm/drm_auth.h>
#include <drm/drm_crtc.h>
#include <drm/drm_file.h>
#include <drm/drm_framebuffer.h>
#include <drm/drm_lease.h>
#include <drm/drm_plane.h>

#include "drm_crtc_internal.h"

static int validate_plane_update(const struct drm_plane_update *update, void *data)
{
	struct drm_file *file = data;

	if (!drm_is_current_master(file) || !drm_lease_held(file, update->plane->base.id))
		return -EACCES;
	if (update->crtc && !drm_lease_held(file, update->crtc->base.id))
		return -EACCES;
	return 0;
}

int drm_mode_setplane_with_preparation(struct drm_device *dev,
				       const struct drm_mode_set_plane *args,
				       struct drm_file *file)
{
	struct drm_plane_update update = {
		.crtc_x = args->crtc_x, .crtc_y = args->crtc_y,
		.crtc_w = args->crtc_w, .crtc_h = args->crtc_h,
		.src_x = args->src_x, .src_y = args->src_y,
		.src_w = args->src_w, .src_h = args->src_h,
	};
	struct drm_prepare_owner *owner;
	int ret;

	owner = drm_file_prepare_owner(file);
	if (IS_ERR(owner))
		return PTR_ERR(owner);
	update.plane = drm_plane_find(dev, file, args->plane_id);
	if (!update.plane) {
		ret = -ENOENT;
		goto out;
	}
	if (!update.plane->funcs->update_plane_request) {
		ret = -EOPNOTSUPP;
		goto out;
	}
	if (args->fb_id) {
		update.fb = drm_framebuffer_lookup(dev, file, args->fb_id);
		if (!update.fb) {
			ret = -ENOENT;
			goto out;
		}
		update.crtc = drm_crtc_find(dev, file, args->crtc_id);
		if (!update.crtc) {
			ret = -ENOENT;
			goto out;
		}
	}
	ret = update.plane->funcs->update_plane_request(&update, owner,
						      validate_plane_update, file);
out:
	if (update.fb)
		drm_framebuffer_put(update.fb);
	drm_prepare_owner_put(owner);
	return ret;
}
