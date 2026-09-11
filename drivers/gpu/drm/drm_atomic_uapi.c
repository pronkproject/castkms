/*
 * Copyright (C) 2014 Red Hat
 * Copyright (C) 2014 Intel Corp.
 * Copyright (C) 2018 Intel Corp.
 * Copyright (c) 2020, The Linux Foundation. All rights reserved.
 *
 * Permission is hereby granted, free of charge, to any person obtaining a
 * copy of this software and associated documentation files (the "Software"),
 * to deal in the Software without restriction, including without limitation
 * the rights to use, copy, modify, merge, publish, distribute, sublicense,
 * and/or sell copies of the Software, and to permit persons to whom the
 * Software is furnished to do so, subject to the following conditions:
 *
 * The above copyright notice and this permission notice shall be included in
 * all copies or substantial portions of the Software.
 *
 * THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
 * IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
 * FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
 * THE COPYRIGHT HOLDER(S) OR AUTHOR(S) BE LIABLE FOR ANY CLAIM, DAMAGES OR
 * OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
 * ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
 * OTHER DEALINGS IN THE SOFTWARE.
 *
 * Authors:
 * Rob Clark <robdclark@gmail.com>
 * Daniel Vetter <daniel.vetter@ffwll.ch>
 */

#include <drm/drm_atomic.h>
#include <drm/drm_atomic_helper.h>
#include <drm/drm_atomic_prepare_auth.h>
#include <drm/drm_atomic_prepare_owner.h>
#include <drm/drm_atomic_prepare_submission.h>
#include <drm/drm_atomic_uapi.h>
#include <drm/drm_file.h>
#include <drm/drm_framebuffer.h>
#include <drm/drm_print.h>
#include <drm/drm_drv.h>
#include <drm/drm_writeback.h>
#include <drm/drm_colorop.h>

#include <linux/export.h>
#include <linux/dma-fence.h>
#include <linux/uaccess.h>
#include <linux/sync_file.h>
#include <kunit/visibility.h>

#include "drm_crtc_internal.h"
#include "drm_atomic_prepare_uapi.h"
#include "drm_atomic_user_commit.h"
#include "drm_atomic_user_input.h"
#include "drm_atomic_user_signaling.h"

/**
 * DOC: overview
 *
 * This file contains the marshalling and demarshalling glue for the atomic UAPI
 * in all its forms: The monster ATOMIC IOCTL itself, code for GET_PROPERTY and
 * SET_PROPERTY IOCTLs. Plus interface functions for compatibility helpers and
 * drivers which have special needs to construct their own atomic updates, e.g.
 * for load detect or similar.
 */

/**
 * drm_atomic_set_mode_for_crtc - set mode for CRTC
 * @state: the CRTC whose incoming state to update
 * @mode: kernel-internal mode to use for the CRTC, or NULL to disable
 *
 * Set a mode (originating from the kernel) on the desired CRTC state and update
 * the enable property.
 *
 * RETURNS:
 * Zero on success, error code on failure. Cannot return -EDEADLK.
 */
int drm_atomic_set_mode_for_crtc(struct drm_crtc_state *state,
				 const struct drm_display_mode *mode)
{
	struct drm_crtc *crtc = state->crtc;
	struct drm_mode_modeinfo umode;

	/* Early return for no change. */
	if (mode && memcmp(&state->mode, mode, sizeof(*mode)) == 0)
		return 0;

	drm_property_blob_put(state->mode_blob);
	state->mode_blob = NULL;

	if (mode) {
		struct drm_property_blob *blob;

		drm_mode_convert_to_umode(&umode, mode);
		blob = drm_property_create_blob(crtc->dev,
						sizeof(umode), &umode);
		if (IS_ERR(blob))
			return PTR_ERR(blob);

		drm_mode_copy(&state->mode, mode);

		state->mode_blob = blob;
		state->enable = true;
		drm_dbg_atomic(crtc->dev,
			       "Set [MODE:%s] for [CRTC:%d:%s] state %p\n",
			       mode->name, crtc->base.id, crtc->name, state);
	} else {
		memset(&state->mode, 0, sizeof(state->mode));
		state->enable = false;
		drm_dbg_atomic(crtc->dev,
			       "Set [NOMODE] for [CRTC:%d:%s] state %p\n",
			       crtc->base.id, crtc->name, state);
	}

	return 0;
}
EXPORT_SYMBOL(drm_atomic_set_mode_for_crtc);

/**
 * drm_atomic_set_mode_prop_for_crtc - set mode for CRTC
 * @state: the CRTC whose incoming state to update
 * @blob: pointer to blob property to use for mode
 *
 * Set a mode (originating from a blob property) on the desired CRTC state.
 * This function will take a reference on the blob property for the CRTC state,
 * and release the reference held on the state's existing mode property, if any
 * was set.
 *
 * RETURNS:
 * Zero on success, error code on failure. Cannot return -EDEADLK.
 */
int drm_atomic_set_mode_prop_for_crtc(struct drm_crtc_state *state,
				      struct drm_property_blob *blob)
{
	struct drm_crtc *crtc = state->crtc;

	if (blob == state->mode_blob)
		return 0;

	drm_property_blob_put(state->mode_blob);
	state->mode_blob = NULL;

	memset(&state->mode, 0, sizeof(state->mode));

	if (blob) {
		int ret;

		if (blob->length != sizeof(struct drm_mode_modeinfo)) {
			drm_dbg_atomic(crtc->dev,
				       "[CRTC:%d:%s] bad mode blob length: %zu\n",
				       crtc->base.id, crtc->name,
				       blob->length);
			return -EINVAL;
		}

		ret = drm_mode_convert_umode(crtc->dev,
					     &state->mode, blob->data);
		if (ret) {
			drm_dbg_atomic(crtc->dev,
				       "[CRTC:%d:%s] invalid mode (%s, %pe): " DRM_MODE_FMT "\n",
				       crtc->base.id, crtc->name,
				       drm_get_mode_status_name(state->mode.status),
				       ERR_PTR(ret), DRM_MODE_ARG(&state->mode));
			return -EINVAL;
		}

		state->mode_blob = drm_property_blob_get(blob);
		state->enable = true;
		drm_dbg_atomic(crtc->dev,
			       "Set [MODE:%s] for [CRTC:%d:%s] state %p\n",
			       state->mode.name, crtc->base.id, crtc->name,
			       state);
	} else {
		state->enable = false;
		drm_dbg_atomic(crtc->dev,
			       "Set [NOMODE] for [CRTC:%d:%s] state %p\n",
			       crtc->base.id, crtc->name, state);
	}

	return 0;
}
EXPORT_SYMBOL(drm_atomic_set_mode_prop_for_crtc);

/**
 * drm_atomic_set_crtc_for_plane - set CRTC for plane
 * @plane_state: the plane whose incoming state to update
 * @crtc: CRTC to use for the plane
 *
 * Changing the assigned CRTC for a plane requires us to grab the lock and state
 * for the new CRTC, as needed. This function takes care of all these details
 * besides updating the pointer in the state object itself.
 *
 * Returns:
 * 0 on success or can fail with -EDEADLK or -ENOMEM. When the error is EDEADLK
 * then the w/w mutex code has detected a deadlock and the entire atomic
 * sequence must be restarted. All other errors are fatal.
 */
int
drm_atomic_set_crtc_for_plane(struct drm_plane_state *plane_state,
			      struct drm_crtc *crtc)
{
	struct drm_plane *plane = plane_state->plane;
	struct drm_crtc_state *crtc_state;
	/* Nothing to do for same crtc*/
	if (plane_state->crtc == crtc)
		return 0;
	if (plane_state->crtc) {
		crtc_state = drm_atomic_get_crtc_state(plane_state->state,
						       plane_state->crtc);
		if (WARN_ON(IS_ERR(crtc_state)))
			return PTR_ERR(crtc_state);

		crtc_state->plane_mask &= ~drm_plane_mask(plane);
	}

	plane_state->crtc = crtc;

	if (crtc) {
		crtc_state = drm_atomic_get_crtc_state(plane_state->state,
						       crtc);
		if (IS_ERR(crtc_state))
			return PTR_ERR(crtc_state);
		crtc_state->plane_mask |= drm_plane_mask(plane);
	}

	if (crtc)
		drm_dbg_atomic(plane->dev,
			       "Link [PLANE:%d:%s] state %p to [CRTC:%d:%s]\n",
			       plane->base.id, plane->name, plane_state,
			       crtc->base.id, crtc->name);
	else
		drm_dbg_atomic(plane->dev,
			       "Link [PLANE:%d:%s] state %p to [NOCRTC]\n",
			       plane->base.id, plane->name, plane_state);

	return 0;
}
EXPORT_SYMBOL(drm_atomic_set_crtc_for_plane);

/**
 * drm_atomic_set_fb_for_plane - set framebuffer for plane
 * @plane_state: atomic state object for the plane
 * @fb: fb to use for the plane
 *
 * Changing the assigned framebuffer for a plane requires us to grab a reference
 * to the new fb and drop the reference to the old fb, if there is one. This
 * function takes care of all these details besides updating the pointer in the
 * state object itself.
 *
 * The assignment sets &drm_plane_state.fb_set even when @fb is unchanged.
 * Both kernel callers and the FB_ID property adapter use this function.
 */
void
drm_atomic_set_fb_for_plane(struct drm_plane_state *plane_state,
			    struct drm_framebuffer *fb)
{
	struct drm_plane *plane = plane_state->plane;

	if (fb)
		drm_dbg_atomic(plane->dev,
			       "Set [FB:%d] for [PLANE:%d:%s] state %p\n",
			       fb->base.id, plane->base.id, plane->name,
			       plane_state);
	else
		drm_dbg_atomic(plane->dev,
			       "Set [NOFB] for [PLANE:%d:%s] state %p\n",
			       plane->base.id, plane->name, plane_state);

	drm_framebuffer_assign(&plane_state->fb, fb);
	plane_state->fb_set = true;
}
EXPORT_SYMBOL(drm_atomic_set_fb_for_plane);

/**
 * drm_atomic_set_colorop_for_plane - set colorop for plane
 * @plane_state: atomic state object for the plane
 * @colorop: colorop to use for the plane
 *
 * Helper function to select the color pipeline on a plane by setting
 * it to the first drm_colorop element of the pipeline.
 *
 * Return: true if plane color pipeline value changed, false otherwise.
 */
bool
drm_atomic_set_colorop_for_plane(struct drm_plane_state *plane_state,
				 struct drm_colorop *colorop)
{
	struct drm_plane *plane = plane_state->plane;

	/* Color pipeline didn't change */
	if (plane_state->color_pipeline == colorop)
		return false;

	if (colorop)
		drm_dbg_atomic(plane->dev,
			       "Set [COLOROP:%d] for [PLANE:%d:%s] state %p\n",
			       colorop->base.id, plane->base.id, plane->name,
			       plane_state);
	else
		drm_dbg_atomic(plane->dev,
			       "Set [NOCOLOROP] for [PLANE:%d:%s] state %p\n",
			       plane->base.id, plane->name, plane_state);

	plane_state->color_pipeline = colorop;

	return true;
}
EXPORT_SYMBOL(drm_atomic_set_colorop_for_plane);

/**
 * drm_atomic_set_crtc_for_connector - set CRTC for connector
 * @conn_state: atomic state object for the connector
 * @crtc: CRTC to use for the connector
 *
 * Changing the assigned CRTC for a connector requires us to grab the lock and
 * state for the new CRTC, as needed. This function takes care of all these
 * details besides updating the pointer in the state object itself.
 *
 * Returns:
 * 0 on success or can fail with -EDEADLK or -ENOMEM. When the error is EDEADLK
 * then the w/w mutex code has detected a deadlock and the entire atomic
 * sequence must be restarted. All other errors are fatal.
 */
int
drm_atomic_set_crtc_for_connector(struct drm_connector_state *conn_state,
				  struct drm_crtc *crtc)
{
	struct drm_connector *connector = conn_state->connector;
	struct drm_crtc_state *crtc_state;

	if (conn_state->crtc == crtc)
		return 0;

	if (conn_state->crtc) {
		crtc_state = drm_atomic_get_new_crtc_state(conn_state->state,
							   conn_state->crtc);

		crtc_state->connector_mask &=
			~drm_connector_mask(conn_state->connector);

		drm_connector_put(conn_state->connector);
		conn_state->crtc = NULL;
	}

	if (crtc) {
		crtc_state = drm_atomic_get_crtc_state(conn_state->state, crtc);
		if (IS_ERR(crtc_state))
			return PTR_ERR(crtc_state);

		crtc_state->connector_mask |=
			drm_connector_mask(conn_state->connector);

		drm_connector_get(conn_state->connector);
		conn_state->crtc = crtc;

		drm_dbg_atomic(connector->dev,
			       "Link [CONNECTOR:%d:%s] state %p to [CRTC:%d:%s]\n",
			       connector->base.id, connector->name,
			       conn_state, crtc->base.id, crtc->name);
	} else {
		drm_dbg_atomic(connector->dev,
			       "Link [CONNECTOR:%d:%s] state %p to [NOCRTC]\n",
			       connector->base.id, connector->name,
			       conn_state);
	}

	return 0;
}
EXPORT_SYMBOL(drm_atomic_set_crtc_for_connector);

static void set_out_fence_for_crtc(struct drm_atomic_commit *state,
				   struct drm_crtc *crtc, s32 __user *fence_ptr)
{
	state->crtcs[drm_crtc_index(crtc)].out_fence_ptr = fence_ptr;
}

static int set_out_fence_for_connector(struct drm_atomic_commit *state,
					struct drm_connector *connector,
					s32 __user *fence_ptr)
{
	unsigned int index = drm_connector_index(connector);

	if (!fence_ptr)
		return 0;

	if (put_user(-1, fence_ptr))
		return -EFAULT;

	state->connectors[index].out_fence_ptr = fence_ptr;

	return 0;
}

static int drm_atomic_crtc_set_property(struct drm_crtc *crtc,
		struct drm_crtc_state *state, struct drm_property *property,
		uint64_t val)
{
	struct drm_device *dev = crtc->dev;
	struct drm_mode_config *config = &dev->mode_config;
	int ret;

	if (property == config->prop_active)
		state->active = val;
	else if (property == config->prop_mode_id) {
		struct drm_property_blob *mode =
			drm_property_lookup_blob(dev, val);
		ret = drm_atomic_set_mode_prop_for_crtc(state, mode);
		drm_property_blob_put(mode);
		return ret;
	} else if (property == config->prop_vrr_enabled) {
		state->vrr_enabled = val;
	} else if (drm_atomic_is_crtc_color_property(crtc, property)) {
		struct drm_property_blob *blob = NULL;

		if (val) {
			blob = drm_property_lookup_blob(dev, val);
			if (!blob)
				return -EINVAL;
		}
		ret = drm_atomic_set_color_property_for_crtc(state, property, blob);
		drm_property_blob_put(blob);
		return ret;
	} else if (property == config->background_color_property) {
		state->background_color = val;
	} else if (property == config->prop_out_fence_ptr) {
		s32 __user *fence_ptr = u64_to_user_ptr(val);

		if (!fence_ptr)
			return 0;

		if (put_user(-1, fence_ptr))
			return -EFAULT;

		set_out_fence_for_crtc(state->state, crtc, fence_ptr);
	} else if (property == crtc->scaling_filter_property) {
		state->scaling_filter = val;
	} else if (property == crtc->sharpness_strength_property) {
		state->sharpness_strength = val;
	} else if (crtc->funcs->atomic_set_property) {
		return crtc->funcs->atomic_set_property(crtc, state, property, val);
	} else {
		drm_dbg_atomic(crtc->dev,
			       "[CRTC:%d:%s] unknown property [PROP:%d:%s]\n",
			       crtc->base.id, crtc->name,
			       property->base.id, property->name);
		return -EINVAL;
	}

	return 0;
}

static int
drm_atomic_crtc_get_property(struct drm_crtc *crtc,
		const struct drm_crtc_state *state,
		struct drm_property *property, uint64_t *val)
{
	struct drm_device *dev = crtc->dev;
	struct drm_mode_config *config = &dev->mode_config;

	if (property == config->prop_active)
		*val = drm_atomic_crtc_effectively_active(state);
	else if (property == config->prop_mode_id)
		*val = (state->mode_blob) ? state->mode_blob->base.id : 0;
	else if (property == config->prop_vrr_enabled)
		*val = state->vrr_enabled;
	else if (property == config->degamma_lut_property)
		*val = (state->degamma_lut) ? state->degamma_lut->base.id : 0;
	else if (property == config->ctm_property)
		*val = (state->ctm) ? state->ctm->base.id : 0;
	else if (property == config->gamma_lut_property)
		*val = (state->gamma_lut) ? state->gamma_lut->base.id : 0;
	else if (property == config->background_color_property)
		*val = state->background_color;
	else if (property == config->prop_out_fence_ptr)
		*val = 0;
	else if (property == config->prop_prepare_fd)
		*val = U64_MAX;
	else if (property == crtc->scaling_filter_property)
		*val = state->scaling_filter;
	else if (property == crtc->sharpness_strength_property)
		*val = state->sharpness_strength;
	else if (crtc->funcs->atomic_get_property)
		return crtc->funcs->atomic_get_property(crtc, state, property, val);
	else {
		drm_dbg_atomic(dev,
			       "[CRTC:%d:%s] unknown property [PROP:%d:%s]\n",
			       crtc->base.id, crtc->name,
			       property->base.id, property->name);
		return -EINVAL;
	}

	return 0;
}

/**
 * drm_atomic_set_fence_for_plane - retain a resolved input fence
 * @plane_state: caller-owned incoming plane state
 * @fence: input fence, or NULL for no explicit dependency
 *
 * Takes a reference without consuming the caller's reference. An existing
 * fence cannot be replaced, even with NULL. Passing NULL to a state without
 * a fence is a no-op. The caller must serialize access to the incoming state.
 * Normal plane-state destruction releases the retained reference.
 *
 * Request builders may retain a resolved fence across discarded attempts and
 * supply it to each fresh state without looking up a userspace descriptor.
 *
 * Returns: zero on success or -EINVAL if the state already has a fence.
 */
int drm_atomic_set_fence_for_plane(struct drm_plane_state *plane_state,
				 struct dma_fence *fence)
{
	if (plane_state->fence)
		return -EINVAL;
	plane_state->fence = dma_fence_get(fence);
	return 0;
}
EXPORT_SYMBOL(drm_atomic_set_fence_for_plane);

static int drm_atomic_plane_set_property(struct drm_plane *plane,
		struct drm_plane_state *state, struct drm_file *file_priv,
		struct drm_property *property, uint64_t val)
{
	struct drm_device *dev = plane->dev;
	struct drm_mode_config *config = &dev->mode_config;
	bool replaced = false;
	int ret;

	if (drm_atomic_is_plane_geometry_property(plane, property))
		return drm_atomic_set_geometry_property_for_plane(state, property, val);

	if (property == config->prop_fb_id) {
		struct drm_framebuffer *fb;

		fb = drm_framebuffer_lookup(dev, file_priv, val);
		drm_atomic_set_fb_for_plane(state, fb);
		if (fb)
			drm_framebuffer_put(fb);
	} else if (property == config->prop_in_fence_fd) {
		struct dma_fence *fence;

		if (state->fence)
			return -EINVAL;

		if (U642I64(val) == -1)
			return drm_atomic_set_fence_for_plane(state, NULL);

		fence = sync_file_get_fence(val);
		if (!fence)
			return -EINVAL;
		ret = drm_atomic_set_fence_for_plane(state, fence);
		dma_fence_put(fence);
		return ret;
	} else if (property == config->prop_crtc_id) {
		struct drm_crtc *crtc = drm_crtc_find(dev, file_priv, val);

		if (val && !crtc) {
			drm_dbg_atomic(dev,
				       "[PROP:%d:%s] cannot find CRTC with ID %llu\n",
				       property->base.id, property->name, val);
			return -EACCES;
		}
		return drm_atomic_set_crtc_for_plane(state, crtc);
	} else if (property == plane->alpha_property) {
		state->alpha = val;
	} else if (property == plane->blend_mode_property) {
		state->pixel_blend_mode = val;
	} else if (property == plane->rotation_property) {
		return drm_atomic_set_rotation_for_plane(state, val);
	} else if (property == plane->zpos_property) {
		state->zpos = val;
	} else if (drm_atomic_is_plane_color_property(plane, property)) {
		return drm_atomic_set_color_property_for_plane(state, property, val);
	} else if (property == plane->color_pipeline_property) {
		/* find DRM colorop object */
		struct drm_colorop *colorop = NULL;

		colorop = drm_colorop_find(dev, file_priv, val);

		if (val && !colorop)
			return -EACCES;

		state->color_mgmt_changed |= drm_atomic_set_colorop_for_plane(state, colorop);
	} else if (property == config->prop_fb_damage_clips) {
		ret = drm_property_replace_blob_from_id(dev,
					&state->fb_damage_clips,
					val,
					-1, -1, sizeof(struct drm_mode_rect),
					&replaced);
		return ret;
	} else if (property == plane->scaling_filter_property) {
		state->scaling_filter = val;
	} else if (plane->funcs->atomic_set_property) {
		return plane->funcs->atomic_set_property(plane, state,
				property, val);
	} else if (property == plane->hotspot_x_property) {
		if (plane->type != DRM_PLANE_TYPE_CURSOR) {
			drm_dbg_atomic(plane->dev,
				       "[PLANE:%d:%s] is not a cursor plane: 0x%llx\n",
				       plane->base.id, plane->name, val);
			return -EINVAL;
		}
		state->hotspot_x = val;
	} else if (property == plane->hotspot_y_property) {
		if (plane->type != DRM_PLANE_TYPE_CURSOR) {
			drm_dbg_atomic(plane->dev,
				       "[PLANE:%d:%s] is not a cursor plane: 0x%llx\n",
				       plane->base.id, plane->name, val);
			return -EINVAL;
		}
		state->hotspot_y = val;
	} else {
		drm_dbg_atomic(plane->dev,
			       "[PLANE:%d:%s] unknown property [PROP:%d:%s]\n",
			       plane->base.id, plane->name,
			       property->base.id, property->name);
		return -EINVAL;
	}

	return 0;
}

static int
drm_atomic_plane_get_property(struct drm_plane *plane,
		const struct drm_plane_state *state,
		struct drm_property *property, uint64_t *val)
{
	struct drm_device *dev = plane->dev;
	struct drm_mode_config *config = &dev->mode_config;

	if (property == config->prop_fb_id) {
		*val = (state->fb) ? state->fb->base.id : 0;
	} else if (property == config->prop_in_fence_fd) {
		*val = -1;
	} else if (property == config->prop_crtc_id) {
		*val = (state->crtc) ? state->crtc->base.id : 0;
	} else if (property == config->prop_crtc_x) {
		*val = I642U64(state->crtc_x);
	} else if (property == config->prop_crtc_y) {
		*val = I642U64(state->crtc_y);
	} else if (property == config->prop_crtc_w) {
		*val = state->crtc_w;
	} else if (property == config->prop_crtc_h) {
		*val = state->crtc_h;
	} else if (property == config->prop_src_x) {
		*val = state->src_x;
	} else if (property == config->prop_src_y) {
		*val = state->src_y;
	} else if (property == config->prop_src_w) {
		*val = state->src_w;
	} else if (property == config->prop_src_h) {
		*val = state->src_h;
	} else if (property == plane->alpha_property) {
		*val = state->alpha;
	} else if (property == plane->blend_mode_property) {
		*val = state->pixel_blend_mode;
	} else if (property == plane->rotation_property) {
		*val = state->rotation;
	} else if (property == plane->zpos_property) {
		*val = state->zpos;
	} else if (property == plane->color_encoding_property) {
		*val = state->color_encoding;
	} else if (property == plane->color_range_property) {
		*val = state->color_range;
	} else if (property == plane->color_pipeline_property) {
		*val = (state->color_pipeline) ? state->color_pipeline->base.id : 0;
	} else if (property == config->prop_fb_damage_clips) {
		*val = (state->fb_damage_clips) ?
			state->fb_damage_clips->base.id : 0;
	} else if (property == plane->scaling_filter_property) {
		*val = state->scaling_filter;
	} else if (plane->funcs->atomic_get_property) {
		return plane->funcs->atomic_get_property(plane, state, property, val);
	} else if (property == plane->hotspot_x_property) {
		*val = state->hotspot_x;
	} else if (property == plane->hotspot_y_property) {
		*val = state->hotspot_y;
	} else {
		drm_dbg_atomic(dev,
			       "[PLANE:%d:%s] unknown property [PROP:%d:%s]\n",
			       plane->base.id, plane->name,
			       property->base.id, property->name);
		return -EINVAL;
	}

	return 0;
}

static int drm_atomic_color_set_data_property(struct drm_colorop *colorop,
					      struct drm_colorop_state *state,
					      struct drm_property *property,
					      uint64_t val,
					      bool *replaced)
{
	ssize_t elem_size = -1;
	ssize_t size = -1;

	switch (colorop->type) {
	case DRM_COLOROP_1D_LUT:
		size = colorop->size * sizeof(struct drm_color_lut32);
		break;
	case DRM_COLOROP_CTM_3X4:
		size = sizeof(struct drm_color_ctm_3x4);
		break;
	case DRM_COLOROP_3D_LUT:
		size = colorop->size * colorop->size * colorop->size *
		       sizeof(struct drm_color_lut32);
		break;
	default:
		/* should never get here */
		return -EINVAL;
	}

	return drm_property_replace_blob_from_id(colorop->dev,
						 &state->data,
						 val,
						 -1, size, elem_size,
						 replaced);
}

static int drm_atomic_colorop_set_property(struct drm_colorop *colorop,
					   struct drm_colorop_state *state,
					   struct drm_file *file_priv,
					   struct drm_property *property,
					   uint64_t val,
					   bool *replaced)
{
	if (property == colorop->bypass_property) {
		if (state->bypass != val) {
			state->bypass = val;
			*replaced = true;
		}
	} else if (property == colorop->lut1d_interpolation_property) {
		if (state->lut1d_interpolation != val) {
			state->lut1d_interpolation = val;
			*replaced = true;
		}
	} else if (property == colorop->curve_1d_type_property) {
		if (state->curve_1d_type != val) {
			state->curve_1d_type = val;
			*replaced = true;
		}
	} else if (property == colorop->multiplier_property) {
		if (state->multiplier != val) {
			state->multiplier = val;
			*replaced = true;
		}
	} else if (property == colorop->lut3d_interpolation_property) {
		if (state->lut3d_interpolation != val) {
			state->lut3d_interpolation = val;
			*replaced = true;
		}
	} else if (property == colorop->data_property) {
		return drm_atomic_color_set_data_property(colorop, state,
							  property, val,
							  replaced);
	} else {
		drm_dbg_atomic(colorop->dev,
			       "[COLOROP:%d:%d] unknown property [PROP:%d:%s]\n",
			       colorop->base.id, colorop->type,
			       property->base.id, property->name);
		return -EINVAL;
	}

	return 0;
}

static int
drm_atomic_colorop_get_property(struct drm_colorop *colorop,
				const struct drm_colorop_state *state,
				struct drm_property *property, uint64_t *val)
{
	if (property == colorop->type_property)
		*val = colorop->type;
	else if (property == colorop->bypass_property)
		*val = state->bypass;
	else if (property == colorop->lut1d_interpolation_property)
		*val = state->lut1d_interpolation;
	else if (property == colorop->curve_1d_type_property)
		*val = state->curve_1d_type;
	else if (property == colorop->multiplier_property)
		*val = state->multiplier;
	else if (property == colorop->size_property)
		*val = colorop->size;
	else if (property == colorop->lut3d_interpolation_property)
		*val = state->lut3d_interpolation;
	else if (property == colorop->data_property)
		*val = (state->data) ? state->data->base.id : 0;
	else
		return -EINVAL;

	return 0;
}

static int drm_atomic_set_writeback_fb_for_connector(
		struct drm_connector_state *conn_state,
		struct drm_framebuffer *fb)
{
	int ret;
	struct drm_connector *conn = conn_state->connector;

	ret = drm_writeback_set_fb(conn_state, fb);
	if (ret < 0)
		return ret;

	if (fb)
		drm_dbg_atomic(conn->dev,
			       "Set [FB:%d] for connector state %p\n",
			       fb->base.id, conn_state);
	else
		drm_dbg_atomic(conn->dev,
			       "Set [NOFB] for connector state %p\n",
			       conn_state);

	return 0;
}

static int drm_atomic_connector_set_property(struct drm_connector *connector,
		struct drm_connector_state *state, struct drm_file *file_priv,
		struct drm_property *property, uint64_t val)
{
	struct drm_device *dev = connector->dev;
	struct drm_mode_config *config = &dev->mode_config;
	bool replaced = false;
	int ret;

	if (property == config->prop_crtc_id) {
		struct drm_crtc *crtc = drm_crtc_find(dev, file_priv, val);

		if (val && !crtc) {
			drm_dbg_atomic(dev,
				       "[PROP:%d:%s] cannot find CRTC with ID %llu\n",
				       property->base.id, property->name, val);
			return -EACCES;
		}
		return drm_atomic_set_crtc_for_connector(state, crtc);
	} else if (property == config->dpms_property) {
		/* setting DPMS property requires special handling, which
		 * is done in legacy setprop path for us.  Disallow (for
		 * now?) atomic writes to DPMS property:
		 */
		drm_dbg_atomic(dev,
			       "legacy [PROP:%d:%s] can only be set via legacy uAPI\n",
			       property->base.id, property->name);
		return -EINVAL;
	} else if (property == config->tv_select_subconnector_property) {
		state->tv.select_subconnector = val;
	} else if (property == config->tv_subconnector_property) {
		state->tv.subconnector = val;
	} else if (property == config->tv_left_margin_property) {
		state->tv.margins.left = val;
	} else if (property == config->tv_right_margin_property) {
		state->tv.margins.right = val;
	} else if (property == config->tv_top_margin_property) {
		state->tv.margins.top = val;
	} else if (property == config->tv_bottom_margin_property) {
		state->tv.margins.bottom = val;
	} else if (property == config->legacy_tv_mode_property) {
		state->tv.legacy_mode = val;
	} else if (property == config->tv_mode_property) {
		state->tv.mode = val;
	} else if (property == config->tv_brightness_property) {
		state->tv.brightness = val;
	} else if (property == config->tv_contrast_property) {
		state->tv.contrast = val;
	} else if (property == config->tv_flicker_reduction_property) {
		state->tv.flicker_reduction = val;
	} else if (property == config->tv_overscan_property) {
		state->tv.overscan = val;
	} else if (property == config->tv_saturation_property) {
		state->tv.saturation = val;
	} else if (property == config->tv_hue_property) {
		state->tv.hue = val;
	} else if (property == config->link_status_property) {
		/* Never downgrade from GOOD to BAD on userspace's request here,
		 * only hw issues can do that.
		 *
		 * For an atomic property the userspace doesn't need to be able
		 * to understand all the properties, but needs to be able to
		 * restore the state it wants on VT switch. So if the userspace
		 * tries to change the link_status from GOOD to BAD, driver
		 * silently rejects it and returns a 0. This prevents userspace
		 * from accidentally breaking  the display when it restores the
		 * state.
		 */
		if (state->link_status != DRM_LINK_STATUS_GOOD)
			state->link_status = val;
	} else if (property == config->hdr_output_metadata_property) {
		ret = drm_property_replace_blob_from_id(dev,
				&state->hdr_output_metadata,
				val,
				-1, sizeof(struct hdr_output_metadata), -1,
				&replaced);
		return ret;
	} else if (property == config->aspect_ratio_property) {
		state->picture_aspect_ratio = val;
	} else if (property == config->content_type_property) {
		state->content_type = val;
	} else if (property == connector->scaling_mode_property) {
		state->scaling_mode = val;
	} else if (property == config->content_protection_property) {
		if (val == DRM_MODE_CONTENT_PROTECTION_ENABLED) {
			drm_dbg_kms(dev, "only drivers can set CP Enabled\n");
			return -EINVAL;
		}
		state->content_protection = val;
	} else if (property == config->hdcp_content_type_property) {
		state->hdcp_content_type = val;
	} else if (property == connector->colorspace_property) {
		state->colorspace = val;
	} else if (property == config->writeback_fb_id_property) {
		struct drm_framebuffer *fb;
		int ret;

		fb = drm_framebuffer_lookup(dev, file_priv, val);
		ret = drm_atomic_set_writeback_fb_for_connector(state, fb);
		if (fb)
			drm_framebuffer_put(fb);
		return ret;
	} else if (property == config->writeback_out_fence_ptr_property) {
		s32 __user *fence_ptr = u64_to_user_ptr(val);

		return set_out_fence_for_connector(state->state, connector,
						   fence_ptr);
	} else if (property == connector->max_bpc_property) {
		state->max_requested_bpc = val;
	} else if (property == connector->privacy_screen_sw_state_property) {
		state->privacy_screen_sw_state = val;
	} else if (property == connector->broadcast_rgb_property) {
		state->hdmi.broadcast_rgb = val;
	} else if (property == connector->color_format_property) {
		state->color_format = val;
	} else if (connector->funcs->atomic_set_property) {
		return connector->funcs->atomic_set_property(connector,
				state, property, val);
	} else {
		drm_dbg_atomic(connector->dev,
			       "[CONNECTOR:%d:%s] unknown property [PROP:%d:%s]\n",
			       connector->base.id, connector->name,
			       property->base.id, property->name);
		return -EINVAL;
	}

	return 0;
}

static int
drm_atomic_connector_get_property(struct drm_connector *connector,
		const struct drm_connector_state *state,
		struct drm_property *property, uint64_t *val)
{
	struct drm_device *dev = connector->dev;
	struct drm_mode_config *config = &dev->mode_config;

	if (property == config->prop_crtc_id) {
		*val = (state->crtc) ? state->crtc->base.id : 0;
	} else if (property == config->dpms_property) {
		if (state->crtc && state->crtc->state->self_refresh_active)
			*val = DRM_MODE_DPMS_ON;
		else
			*val = connector->dpms;
	} else if (property == config->tv_select_subconnector_property) {
		*val = state->tv.select_subconnector;
	} else if (property == config->tv_subconnector_property) {
		*val = state->tv.subconnector;
	} else if (property == config->tv_left_margin_property) {
		*val = state->tv.margins.left;
	} else if (property == config->tv_right_margin_property) {
		*val = state->tv.margins.right;
	} else if (property == config->tv_top_margin_property) {
		*val = state->tv.margins.top;
	} else if (property == config->tv_bottom_margin_property) {
		*val = state->tv.margins.bottom;
	} else if (property == config->legacy_tv_mode_property) {
		*val = state->tv.legacy_mode;
	} else if (property == config->tv_mode_property) {
		*val = state->tv.mode;
	} else if (property == config->tv_brightness_property) {
		*val = state->tv.brightness;
	} else if (property == config->tv_contrast_property) {
		*val = state->tv.contrast;
	} else if (property == config->tv_flicker_reduction_property) {
		*val = state->tv.flicker_reduction;
	} else if (property == config->tv_overscan_property) {
		*val = state->tv.overscan;
	} else if (property == config->tv_saturation_property) {
		*val = state->tv.saturation;
	} else if (property == config->tv_hue_property) {
		*val = state->tv.hue;
	} else if (property == config->link_status_property) {
		*val = state->link_status;
	} else if (property == config->aspect_ratio_property) {
		*val = state->picture_aspect_ratio;
	} else if (property == config->content_type_property) {
		*val = state->content_type;
	} else if (property == connector->colorspace_property) {
		*val = state->colorspace;
	} else if (property == connector->scaling_mode_property) {
		*val = state->scaling_mode;
	} else if (property == config->hdr_output_metadata_property) {
		*val = state->hdr_output_metadata ?
			state->hdr_output_metadata->base.id : 0;
	} else if (property == config->content_protection_property) {
		*val = state->content_protection;
	} else if (property == config->hdcp_content_type_property) {
		*val = state->hdcp_content_type;
	} else if (property == config->writeback_fb_id_property) {
		/* Writeback framebuffer is one-shot, write and forget */
		*val = 0;
	} else if (property == config->writeback_out_fence_ptr_property) {
		*val = 0;
	} else if (property == connector->max_bpc_property) {
		*val = state->max_requested_bpc;
	} else if (property == connector->privacy_screen_sw_state_property) {
		*val = state->privacy_screen_sw_state;
	} else if (property == connector->broadcast_rgb_property) {
		*val = state->hdmi.broadcast_rgb;
	} else if (property == connector->color_format_property) {
		*val = state->color_format;
	} else if (connector->funcs->atomic_get_property) {
		return connector->funcs->atomic_get_property(connector,
				state, property, val);
	} else {
		drm_dbg_atomic(dev,
			       "[CONNECTOR:%d:%s] unknown property [PROP:%d:%s]\n",
			       connector->base.id, connector->name,
			       property->base.id, property->name);
		return -EINVAL;
	}

	return 0;
}

int drm_atomic_get_property(struct drm_mode_object *obj,
		struct drm_property *property, uint64_t *val)
{
	struct drm_device *dev = property->dev;
	int ret;

	switch (obj->type) {
	case DRM_MODE_OBJECT_CONNECTOR: {
		struct drm_connector *connector = obj_to_connector(obj);

		WARN_ON(!drm_modeset_is_locked(&dev->mode_config.connection_mutex));
		ret = drm_atomic_connector_get_property(connector,
				connector->state, property, val);
		break;
	}
	case DRM_MODE_OBJECT_CRTC: {
		struct drm_crtc *crtc = obj_to_crtc(obj);

		WARN_ON(!drm_modeset_is_locked(&crtc->mutex));
		ret = drm_atomic_crtc_get_property(crtc,
				crtc->state, property, val);
		break;
	}
	case DRM_MODE_OBJECT_PLANE: {
		struct drm_plane *plane = obj_to_plane(obj);

		WARN_ON(!drm_modeset_is_locked(&plane->mutex));
		ret = drm_atomic_plane_get_property(plane,
				plane->state, property, val);
		break;
	}
	case DRM_MODE_OBJECT_COLOROP: {
		struct drm_colorop *colorop = obj_to_colorop(obj);

		if (colorop->plane)
			WARN_ON(!drm_modeset_is_locked(&colorop->plane->mutex));

		ret = drm_atomic_colorop_get_property(colorop, colorop->state, property, val);
		break;
	}
	default:
		drm_dbg_atomic(dev, "[OBJECT:%d] has no properties\n", obj->id);
		ret = -EINVAL;
		break;
	}

	return ret;
}

/*
 * The big monster ioctl
 */

int drm_atomic_connector_commit_dpms(struct drm_atomic_commit *state,
				     struct drm_connector *connector,
				     int mode)
{
	struct drm_connector *tmp_connector;
	struct drm_connector_state *new_conn_state;
	struct drm_crtc *crtc;
	struct drm_crtc_state *crtc_state;
	int i, ret, old_mode = connector->dpms;
	bool active = false;

	ret = drm_modeset_lock(&state->dev->mode_config.connection_mutex,
			       state->acquire_ctx);
	if (ret)
		return ret;

	if (mode != DRM_MODE_DPMS_ON)
		mode = DRM_MODE_DPMS_OFF;

	if (connector->dpms == mode)
		goto out;

	connector->dpms = mode;

	crtc = connector->state->crtc;
	if (!crtc)
		goto out;
	ret = drm_atomic_add_affected_connectors(state, crtc);
	if (ret)
		goto out;

	crtc_state = drm_atomic_get_crtc_state(state, crtc);
	if (IS_ERR(crtc_state)) {
		ret = PTR_ERR(crtc_state);
		goto out;
	}

	for_each_new_connector_in_state(state, tmp_connector, new_conn_state, i) {
		if (new_conn_state->crtc != crtc)
			continue;
		if (tmp_connector->dpms == DRM_MODE_DPMS_ON) {
			active = true;
			break;
		}
	}

	crtc_state->active = active;
	ret = drm_atomic_commit(state);
out:
	if (ret != 0)
		connector->dpms = old_mode;
	return ret;
}

static int drm_atomic_check_prop_changes(int ret, uint64_t old_val, uint64_t prop_value,
					 struct drm_property *prop)
{
	if (ret != 0 || old_val != prop_value) {
		drm_dbg_atomic(prop->dev,
			       "[PROP:%d:%s] No prop can be changed during async flip\n",
			       prop->base.id, prop->name);
		return -EINVAL;
	}

	return 0;
}

int drm_atomic_set_property(struct drm_atomic_commit *state,
			    struct drm_file *file_priv,
			    struct drm_mode_object *obj,
			    struct drm_property *prop,
			    u64 prop_value,
			    bool async_flip)
{
	struct drm_mode_object *ref;
	u64 old_val;
	int ret;

	if (!drm_property_change_valid_get(prop, prop_value, &ref))
		return -EINVAL;

	switch (obj->type) {
	case DRM_MODE_OBJECT_CONNECTOR: {
		struct drm_connector *connector = obj_to_connector(obj);
		struct drm_connector_state *connector_state;

		connector_state = drm_atomic_get_connector_state(state, connector);
		if (IS_ERR(connector_state)) {
			ret = PTR_ERR(connector_state);
			break;
		}

		if (async_flip) {
			ret = drm_atomic_connector_get_property(connector, connector_state,
								prop, &old_val);
			ret = drm_atomic_check_prop_changes(ret, old_val, prop_value, prop);
			break;
		}

		ret = drm_atomic_connector_set_property(connector,
				connector_state, file_priv,
				prop, prop_value);
		break;
	}
	case DRM_MODE_OBJECT_CRTC: {
		struct drm_crtc *crtc = obj_to_crtc(obj);
		struct drm_crtc_state *crtc_state;

		crtc_state = drm_atomic_get_crtc_state(state, crtc);
		if (IS_ERR(crtc_state)) {
			ret = PTR_ERR(crtc_state);
			break;
		}

		if (async_flip) {
			ret = drm_atomic_crtc_get_property(crtc, crtc_state,
							   prop, &old_val);
			ret = drm_atomic_check_prop_changes(ret, old_val, prop_value, prop);
			break;
		}
		if (prop == state->dev->mode_config.prop_prepare_fd) {
			ret = drm_atomic_prepare_set_fd(state, crtc, prop_value);
			break;
		}

		ret = drm_atomic_crtc_set_property(crtc,
				crtc_state, prop, prop_value);
		break;
	}
	case DRM_MODE_OBJECT_PLANE: {
		struct drm_plane *plane = obj_to_plane(obj);
		struct drm_plane_state *plane_state;
		struct drm_mode_config *config = &plane->dev->mode_config;
		const struct drm_plane_helper_funcs *plane_funcs = plane->helper_private;

		plane_state = drm_atomic_get_plane_state(state, plane);
		if (IS_ERR(plane_state)) {
			ret = PTR_ERR(plane_state);
			break;
		}

		if (async_flip) {
			/* no-op changes are always allowed */
			ret = drm_atomic_plane_get_property(plane, plane_state,
							    prop, &old_val);
			ret = drm_atomic_check_prop_changes(ret, old_val, prop_value, prop);

			/* fail everything that isn't no-op or a pure flip */
			if (ret && prop != config->prop_fb_id &&
			    prop != config->prop_in_fence_fd &&
			    prop != config->prop_fb_damage_clips) {
				break;
			}

			if (ret && plane->type != DRM_PLANE_TYPE_PRIMARY) {
				/* ask the driver if this non-primary plane is supported */
				if (plane_funcs && plane_funcs->atomic_async_check)
					ret = plane_funcs->atomic_async_check(plane, state, true);

				if (ret) {
					drm_dbg_atomic(prop->dev,
						       "[PLANE:%d:%s] does not support async flips\n",
						       obj->id, plane->name);
					break;
				}
			}
		}

		ret = drm_atomic_plane_set_property(plane,
				plane_state, file_priv,
				prop, prop_value);

		break;
	}
	case DRM_MODE_OBJECT_COLOROP: {
		struct drm_plane_state *plane_state;
		struct drm_colorop *colorop = obj_to_colorop(obj);
		struct drm_colorop_state *colorop_state;
		bool replaced = false;

		colorop_state = drm_atomic_get_colorop_state(state, colorop);
		if (IS_ERR(colorop_state)) {
			ret = PTR_ERR(colorop_state);
			break;
		}

		ret = drm_atomic_colorop_set_property(colorop, colorop_state,
						      file_priv, prop, prop_value,
						      &replaced);
		if (ret || !replaced)
			break;

		plane_state = drm_atomic_get_plane_state(state, colorop->plane);
		if (IS_ERR(plane_state)) {
			ret = PTR_ERR(plane_state);
			break;
		}
		plane_state->color_mgmt_changed |= replaced;

		break;
	}
	default:
		drm_dbg_atomic(prop->dev, "[OBJECT:%d] has no properties\n", obj->id);
		ret = -EINVAL;
		break;
	}

	drm_property_change_valid_put(prop, ref);
	return ret;
}

/**
 * DOC: explicit fencing properties
 *
 * Explicit fencing allows userspace to control the buffer synchronization
 * between devices. A Fence or a group of fences are transferred to/from
 * userspace using Sync File fds and there are two DRM properties for that.
 * IN_FENCE_FD on each DRM Plane to send fences to the kernel and
 * OUT_FENCE_PTR on each DRM CRTC to receive fences from the kernel.
 *
 * As a contrast, with implicit fencing the kernel keeps track of any
 * ongoing rendering, and automatically ensures that the atomic update waits
 * for any pending rendering to complete. This is usually tracked in &struct
 * dma_resv which can also contain mandatory kernel fences. Implicit syncing
 * is how Linux traditionally worked (e.g. DRI2/3 on X.org), whereas explicit
 * fencing is what Android wants.
 *
 * "IN_FENCE_FD”:
 *	Use this property to pass a fence that DRM should wait on before
 *	proceeding with the Atomic Commit request and show the framebuffer for
 *	the plane on the screen. The fence can be either a normal fence or a
 *	merged one, the sync_file framework will handle both cases and use a
 *	fence_array if a merged fence is received. Passing -1 here means no
 *	fences to wait on.
 *
 *	If the Atomic Commit request has the DRM_MODE_ATOMIC_TEST_ONLY flag
 *	it will only check if the Sync File is a valid one.
 *
 *	On the driver side the fence is stored on the @fence parameter of
 *	&struct drm_plane_state. Drivers which also support implicit fencing
 *	should extract the implicit fence using drm_gem_plane_helper_prepare_fb(),
 *	to make sure there's consistent behaviour between drivers in precedence
 *	of implicit vs. explicit fencing.
 *
 * "OUT_FENCE_PTR”:
 *	Use this property to pass a file descriptor pointer to DRM. Once the
 *	Atomic Commit request call returns OUT_FENCE_PTR will be filled with
 *	the file descriptor number of a Sync File. This Sync File contains the
 *	CRTC fence that will be signaled when all framebuffers present on the
 *	Atomic Commit * request for that given CRTC are scanned out on the
 *	screen.
 *
 *	The Atomic Commit request fails if a invalid pointer is passed. If the
 *	Atomic Commit request fails for any other reason the out fence fd
 *	returned will be -1. On a Atomic Commit with the
 *	DRM_MODE_ATOMIC_TEST_ONLY flag the out fence will also be set to -1.
 *
 *	Note that out-fences don't have a special interface to drivers and are
 *	internally represented by a &struct drm_pending_vblank_event in struct
 *	&drm_crtc_state, which is also used by the nonblocking atomic commit
 *	helpers and for the DRM event handling for existing userspace.
 */

static void
set_async_flip(struct drm_atomic_commit *state)
{
	struct drm_crtc *crtc;
	struct drm_crtc_state *crtc_state;
	int i;

	for_each_new_crtc_in_state(state, crtc, crtc_state, i) {
		crtc_state->async_flip = true;
	}
}

static bool has_prepare_descriptor(struct drm_device *dev,
				   const struct drm_atomic_user_input *input)
{
	unsigned int i;

	for (i = 0; i < input->property_count; i++)
		if (input->properties[i] == dev->mode_config.prop_prepare_fd->base.id)
			return true;
	return false;
}

int drm_mode_atomic_ioctl(struct drm_device *dev,
			  void *data, struct drm_file *file_priv)
{
	struct drm_mode_atomic *arg = data;
	struct drm_atomic_user_input *input;
	unsigned int copied_objs, copied_props;
	struct drm_atomic_commit *state;
	struct drm_modeset_acquire_ctx ctx;
	struct drm_atomic_user_signaling *signaling;
	struct drm_prepare_owner *prepare_owner = NULL;
	int ret = 0;
	unsigned int i, j;
	bool async_flip = false;
	bool prepare_blocking;
	bool explicit_preparation;

	/* disallow for drivers not supporting atomic: */
	if (!drm_core_check_feature(dev, DRIVER_ATOMIC))
		return -EOPNOTSUPP;

	/* disallow for userspace that has not enabled atomic cap (even
	 * though this may be a bit overkill, since legacy userspace
	 * wouldn't know how to call this ioctl)
	 */
	if (!file_priv->atomic) {
		drm_dbg_atomic(dev,
			       "commit failed: atomic cap not enabled\n");
		return -EINVAL;
	}

	if (arg->flags & ~DRM_MODE_ATOMIC_FLAGS) {
		drm_dbg_atomic(dev, "commit failed: invalid flag\n");
		return -EINVAL;
	}

	if (arg->reserved) {
		drm_dbg_atomic(dev, "commit failed: reserved field set\n");
		return -EINVAL;
	}

	if (arg->flags & DRM_MODE_PAGE_FLIP_ASYNC) {
		if (!dev->mode_config.async_page_flip) {
			drm_dbg_atomic(dev,
				       "commit failed: DRM_MODE_PAGE_FLIP_ASYNC not supported\n");
			return -EINVAL;
		}

		async_flip = true;
	}

	/* can't test and expect an event at the same time. */
	if ((arg->flags & DRM_MODE_ATOMIC_TEST_ONLY) &&
			(arg->flags & DRM_MODE_PAGE_FLIP_EVENT)) {
		drm_dbg_atomic(dev,
			       "commit failed: page-flip event requested with test-only commit\n");
		return -EINVAL;
	}

	explicit_preparation = READ_ONCE(file_priv->atomic_preparation);
	prepare_blocking = dev->mode_config.preparation &&
		!(arg->flags & (DRM_MODE_ATOMIC_TEST_ONLY | DRM_MODE_ATOMIC_NONBLOCK |
				DRM_MODE_PAGE_FLIP_ASYNC));
	state = drm_atomic_commit_alloc(dev);
	if (!state)
		return -ENOMEM;
	if (explicit_preparation || prepare_blocking) {
		prepare_owner = drm_file_prepare_owner(file_priv);
		if (IS_ERR(prepare_owner)) {
			ret = PTR_ERR(prepare_owner);
			drm_atomic_commit_put(state);
			return ret;
		}
	}
	input = drm_atomic_copy_user_input(arg);
	if (IS_ERR(input)) {
		ret = PTR_ERR(input);
		drm_atomic_commit_put(state);
		if (prepare_owner)
			drm_prepare_owner_put(prepare_owner);
		return ret;
	}

	if (prepare_blocking && !has_prepare_descriptor(dev, input)) {
		drm_atomic_commit_put(state);
		ret = drm_atomic_commit_user_request(dev, file_priv, prepare_owner,
						     arg->flags, arg->user_data, input);
		goto out_input;
	}

	drm_modeset_acquire_init(&ctx, DRM_MODESET_ACQUIRE_INTERRUPTIBLE);
	state->acquire_ctx = &ctx;
	state->allow_modeset = !!(arg->flags & DRM_MODE_ATOMIC_ALLOW_MODESET);
	state->plane_color_pipeline = file_priv->plane_color_pipeline;

retry:
	copied_objs = 0;
	copied_props = 0;
	signaling = NULL;
	if (explicit_preparation) {
		ret = drm_atomic_prepare_submission_init(state, prepare_owner);
		if (ret)
			goto out;
	}

	for (i = 0; i < input->object_count; i++) {
		uint32_t obj_id, count_props;
		struct drm_mode_object *obj;

		obj_id = input->objects[copied_objs];

		obj = drm_mode_object_find(dev, file_priv, obj_id, DRM_MODE_OBJECT_ANY);
		if (!obj) {
			drm_dbg_atomic(dev, "cannot find object ID %d", obj_id);
			ret = -ENOENT;
			goto out;
		}

		if (!obj->properties) {
			drm_dbg_atomic(dev, "[OBJECT:%d] has no properties", obj_id);
			drm_mode_object_put(obj);
			ret = -ENOENT;
			goto out;
		}

		count_props = input->counts[copied_objs];

		copied_objs++;

		for (j = 0; j < count_props; j++) {
			uint32_t prop_id;
			uint64_t prop_value;
			struct drm_property *prop;

			prop_id = input->properties[copied_props];

			prop = drm_mode_obj_find_prop_id(obj, prop_id);
			if (!prop) {
				drm_dbg_atomic(dev,
					       "[OBJECT:%d] cannot find property ID %d",
					       obj_id, prop_id);
				drm_mode_object_put(obj);
				ret = -ENOENT;
				goto out;
			}

			prop_value = input->values[copied_props];

			ret = drm_atomic_set_property(state, file_priv, obj,
						      prop, prop_value, async_flip);
			if (ret) {
				drm_mode_object_put(obj);
				goto out;
			}

			copied_props++;
		}

		drm_mode_object_put(obj);
	}

	ret = drm_atomic_prepare_user_signaling(state, file_priv, arg->flags,
						arg->user_data, &signaling);
	if (ret)
		goto out;

	if (arg->flags & DRM_MODE_PAGE_FLIP_ASYNC)
		set_async_flip(state);
	if (!(arg->flags & DRM_MODE_ATOMIC_TEST_ONLY)) {
		ret = drm_atomic_prepare_submission_attach(state);
		if (ret)
			goto out;
	}

	if (arg->flags & DRM_MODE_ATOMIC_TEST_ONLY) {
		ret = drm_atomic_check_only(state);
	} else if (arg->flags & DRM_MODE_ATOMIC_NONBLOCK) {
		ret = drm_atomic_nonblocking_commit(state);
	} else {
		ret = drm_atomic_commit(state);
	}

out:
	drm_atomic_complete_user_signaling(state, signaling, !ret);

	if (ret == -EDEADLK) {
		drm_atomic_commit_clear(state);
		ret = drm_modeset_backoff(&ctx);
		if (!ret)
			goto retry;
	}

	drm_atomic_commit_put(state);

	drm_modeset_drop_locks(&ctx);
	drm_modeset_acquire_fini(&ctx);
out_input:
	if (prepare_owner)
		drm_prepare_owner_put(prepare_owner);
	drm_atomic_free_user_input(input);

	return ret;
}
EXPORT_SYMBOL_FOR_TESTS_ONLY(drm_mode_atomic_ioctl);
