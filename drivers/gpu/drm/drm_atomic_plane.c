// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/export.h>
#include <linux/log2.h>
#include <drm/drm_atomic_uapi.h>
#include <drm/drm_crtc.h>
#include <drm/drm_device.h>
#include <drm/drm_plane.h>
#include <drm/drm_property.h>

#include "drm_crtc_internal.h"

bool drm_atomic_is_plane_geometry_property(struct drm_plane *plane,
					   struct drm_property *property)
{
	struct drm_mode_config *config = &plane->dev->mode_config;

	return property &&
		(property == config->prop_crtc_x || property == config->prop_crtc_y ||
		 property == config->prop_crtc_w || property == config->prop_crtc_h ||
		 property == config->prop_src_x || property == config->prop_src_y ||
		 property == config->prop_src_w || property == config->prop_src_h);
}

/**
 * drm_atomic_set_geometry_property_for_plane - set a plane coordinate or size
 * @state: uncommitted plane state protected by its modeset lock
 * @property: one of the plane's CRTC_X/Y/W/H or SRC_X/Y/W/H properties
 * @value: signed display coordinate or unsigned size/source coordinate
 *
 * Validates attachment and the property's range before changing the selected
 * field. Source coordinates and sizes keep their unsigned 16.16 representation.
 * Display coordinates retain their signed interpretation. The caller must still
 * check the complete atomic update for valid geometry and driver constraints.
 * No identifier lookup, file access, or driver property callback is performed.
 *
 * Return: 0 on success, -EOPNOTSUPP for another property, or -EINVAL if the
 * property is not attached or the value is outside its range. Errors do not
 * change @state.
 */
int drm_atomic_set_geometry_property_for_plane(struct drm_plane_state *state,
					       struct drm_property *property, u64 value)
{
	struct drm_plane *plane = state->plane;
	struct drm_mode_config *config = &plane->dev->mode_config;
	struct drm_mode_object *unused;

	if (!drm_atomic_is_plane_geometry_property(plane, property))
		return -EOPNOTSUPP;
	if (drm_mode_obj_find_prop_id(&plane->base, property->base.id) != property ||
	    !drm_property_change_valid_get(property, value, &unused))
		return -EINVAL;

	if (property == config->prop_crtc_x)
		state->crtc_x = U642I64(value);
	else if (property == config->prop_crtc_y)
		state->crtc_y = U642I64(value);
	else if (property == config->prop_crtc_w)
		state->crtc_w = value;
	else if (property == config->prop_crtc_h)
		state->crtc_h = value;
	else if (property == config->prop_src_x)
		state->src_x = value;
	else if (property == config->prop_src_y)
		state->src_y = value;
	else if (property == config->prop_src_w)
		state->src_w = value;
	else if (property == config->prop_src_h)
		state->src_h = value;
	return 0;
}
EXPORT_SYMBOL_GPL(drm_atomic_set_geometry_property_for_plane);

/**
 * drm_atomic_set_rotation_for_plane - set one rotation with optional reflections
 * @state: uncommitted plane state protected by its modeset lock
 * @rotation: exactly one rotation bit, plus any advertised reflection bits
 *
 * Validates the plane's attached rotation property's advertised bits and requires
 * exactly one rotation. Errors leave the state unchanged. The caller must still
 * check the complete update for geometry and driver constraints.
 *
 * Return: 0 on success, -EOPNOTSUPP if the plane has no rotation property, or
 * -EINVAL if the property is detached or the requested combination is invalid.
 */
int drm_atomic_set_rotation_for_plane(struct drm_plane_state *state, u64 rotation)
{
	struct drm_plane *plane = state->plane;
	struct drm_property *property = plane->rotation_property;
	struct drm_mode_object *unused;

	if (!property)
		return -EOPNOTSUPP;
	if (drm_mode_obj_find_prop_id(&plane->base, property->base.id) != property ||
	    !drm_property_change_valid_get(property, rotation, &unused) ||
	    !is_power_of_2(rotation & DRM_MODE_ROTATE_MASK))
		return -EINVAL;
	state->rotation = rotation;
	return 0;
}
EXPORT_SYMBOL_GPL(drm_atomic_set_rotation_for_plane);
