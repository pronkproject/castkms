// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/export.h>
#include <drm/drm_atomic.h>
#include <drm/drm_atomic_request.h>
#include <drm/drm_atomic_uapi.h>
#include <drm/drm_connector.h>
#include <drm/drm_property.h>

#include "drm_crtc_internal.h"

static bool supported_entry(const struct drm_atomic_request_entry *entry)
{
	struct drm_mode_config *config = &entry->property->dev->mode_config;

	if (entry->object->type == DRM_MODE_OBJECT_PLANE)
		return entry->property == config->prop_fb_id ||
		       entry->property == config->prop_in_fence_fd ||
		       entry->property == config->prop_crtc_id ||
		       entry->property == config->prop_fb_damage_clips ||
		       drm_atomic_is_plane_geometry_property(obj_to_plane(entry->object),
						      entry->property);
	if (entry->object->type == DRM_MODE_OBJECT_CRTC)
		return entry->property == config->prop_mode_id ||
		       entry->property == config->prop_active ||
		       drm_atomic_is_crtc_color_property(obj_to_crtc(entry->object), entry->property);
	if (entry->object->type == DRM_MODE_OBJECT_CONNECTOR)
		return entry->property == config->prop_crtc_id;
	return false;
}

static int apply_plane(struct drm_atomic_commit *state,
		       const struct drm_atomic_request_entry *entry)
{
	struct drm_mode_config *config = &state->dev->mode_config;
	struct drm_plane_state *plane_state;
	bool replaced = false;

	plane_state = drm_atomic_get_plane_state(state, obj_to_plane(entry->object));
	if (IS_ERR(plane_state))
		return PTR_ERR(plane_state);
	if (drm_atomic_is_plane_geometry_property(plane_state->plane, entry->property))
		return drm_atomic_set_geometry_property_for_plane(plane_state, entry->property,
								 entry->scalar);
	if (entry->property == config->prop_fb_id) {
		drm_atomic_set_fb_for_plane(plane_state, entry->framebuffer);
		return 0;
	}
	if (entry->property == config->prop_in_fence_fd)
		return drm_atomic_set_fence_for_plane(plane_state, entry->fence);
	if (entry->property == config->prop_crtc_id)
		return drm_atomic_set_crtc_for_plane(plane_state,
				entry->reference ? obj_to_crtc(entry->reference) : NULL);
	if (entry->property == config->prop_fb_damage_clips)
		return drm_property_replace_blob_checked(state->dev,
				&plane_state->fb_damage_clips, entry->blob,
				-1, -1, sizeof(struct drm_mode_rect), &replaced);
	return -EOPNOTSUPP;
}

static int apply_entry(struct drm_atomic_commit *state,
		       const struct drm_atomic_request_entry *entry)
{
	struct drm_crtc_state *crtc_state;
	struct drm_connector_state *connector_state;

	switch (entry->object->type) {
	case DRM_MODE_OBJECT_PLANE:
		return apply_plane(state, entry);
	case DRM_MODE_OBJECT_CRTC:
		crtc_state = drm_atomic_get_crtc_state(state, obj_to_crtc(entry->object));
		if (IS_ERR(crtc_state))
			return PTR_ERR(crtc_state);
		if (entry->property == state->dev->mode_config.prop_active) {
			crtc_state->active = entry->scalar;
			return 0;
		}
		if (drm_atomic_is_crtc_color_property(crtc_state->crtc, entry->property))
			return drm_atomic_set_color_property_for_crtc(crtc_state, entry->property,
								     entry->blob);
		return drm_atomic_set_mode_prop_for_crtc(crtc_state, entry->blob);
	case DRM_MODE_OBJECT_CONNECTOR:
		connector_state = drm_atomic_get_connector_state(state,
								obj_to_connector(entry->object));
		if (IS_ERR(connector_state))
			return PTR_ERR(connector_state);
		return drm_atomic_set_crtc_for_connector(connector_state,
				entry->reference ? obj_to_crtc(entry->reference) : NULL);
	default:
		return -EOPNOTSUPP;
	}
}

/**
 * drm_atomic_request_apply - apply retained values to an uncommitted update
 * @request: immutable request retaining the input references
 * @state: fresh or cleared atomic update with an initialized acquire context
 * @validate: required caller authority and availability check
 * @data: caller data for @validate
 *
 * Acquires all modeset locks, verifies property attachment and the supported
 * property set, then calls @validate before applying any assignment. The caller
 * must check permission for all targets and referenced objects, including any
 * unregistered objects, on every invocation. Retention does not grant access.
 * The callback runs under modeset locks and must not drop them or modify the
 * request or update. The caller keeps authority valid through submission.
 *
 * Entries are applied in order using kernel references, without identifier or
 * descriptor lookup. Supported properties are plane FB_ID, IN_FENCE_FD,
 * CRTC_ID, FB_DAMAGE_CLIPS, CRTC_X/Y/W/H and SRC_X/Y/W/H, controller MODE_ID/ACTIVE,
 * DEGAMMA_LUT/CTM/GAMMA_LUT, and connector CRTC_ID.
 * Driver-private properties and asynchronous-flip validation are not supported.
 * No check or commit runs.
 *
 * Locks remain in the caller's acquire context on every return. On error the
 * caller must clear or discard @state before retrying; assignments are not
 * rolled back. Neither @request nor its references are consumed.
 *
 * Return: 0 on success; -EOPNOTSUPP for unsupported properties; otherwise a
 * negative validation, locking or setter error. Handle -EDEADLK by clearing
 * @state and backing off its acquire context before another attempt.
 */
int drm_atomic_request_apply(const struct drm_atomic_request *request,
			     struct drm_atomic_commit *state,
			     int (*validate)(struct drm_atomic_commit *state,
					     const struct drm_atomic_request *request,
					     void *data),
			     void *data)
{
	unsigned int i;
	int ret;

	if (!state->acquire_ctx || !validate || state->async_update)
		return -EINVAL;
	ret = drm_modeset_lock_all_ctx(state->dev, state->acquire_ctx);
	if (ret)
		return ret;
	for (i = 0; i < drm_atomic_request_count(request); i++) {
		const struct drm_atomic_request_entry *entry = drm_atomic_request_entry(request, i);

		if (entry->property->dev != state->dev ||
		    drm_mode_obj_find_prop_id(entry->object, entry->property->base.id) !=
			entry->property)
			return -EINVAL;
		if (!supported_entry(entry))
			return -EOPNOTSUPP;
	}
	ret = validate(state, request, data);
	if (ret)
		return ret;
	for (i = 0; i < drm_atomic_request_count(request); i++) {
		ret = apply_entry(state, drm_atomic_request_entry(request, i));
		if (ret)
			return ret;
	}
	return 0;
}
EXPORT_SYMBOL_GPL(drm_atomic_request_apply);
