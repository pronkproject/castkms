// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/err.h>
#include <linux/slab.h>
#include <drm/drm_constraints.h>
#include <drm/drm_constraints_list.h>
#include <drm/drm_constraints_entry.h>
#include <drm/drm_constraints_output.h>
#include <drm/drm_crtc.h>
#include <drm/drm_device.h>
#include <drm/drm_drv.h>
#include <drm/drm_property.h>

#include "drm_constraints_internal.h"
#include "drm_crtc_internal.h"

static struct drm_mode_object *find_property_object(struct drm_crtc *crtc, u32 id)
{
	struct drm_plane *plane;

	if (crtc->base.id == id)
		return &crtc->base;
	drm_for_each_plane(plane, crtc->dev)
		if (plane->base.id == id && (plane->possible_crtcs & drm_crtc_mask(crtc)))
			return &plane->base;
	return NULL;
}

static bool scene_property(struct drm_crtc *crtc, struct drm_mode_object *object,
			   struct drm_property *property)
{
	struct drm_mode_config *config = &crtc->dev->mode_config;
	struct drm_plane *plane;

	if (object == &crtc->base)
		return property == config->prop_vrr_enabled ||
			property == config->background_color_property ||
			property == crtc->scaling_filter_property ||
			property == crtc->sharpness_strength_property;
	plane = obj_to_plane(object);
	return property == config->prop_crtc_x || property == config->prop_crtc_y ||
		property == config->prop_crtc_w || property == config->prop_crtc_h ||
		property == config->prop_src_x || property == config->prop_src_y ||
		property == config->prop_src_w || property == config->prop_src_h ||
		property == plane->alpha_property || property == plane->blend_mode_property ||
		property == plane->rotation_property || property == plane->zpos_property ||
		property == plane->color_encoding_property ||
		property == plane->color_range_property ||
		property == plane->scaling_filter_property ||
		property == plane->hotspot_x_property || property == plane->hotspot_y_property;
}

static int validate_property(struct drm_crtc *crtc, const struct drm_constraints_property *rule)
{
	struct drm_mode_object *object = find_property_object(crtc, rule->object_id);
	struct drm_property *property;
	u64 allowed = 0;
	unsigned int i;

	if (!object)
		return -EINVAL;
	property = drm_mode_obj_find_prop_id(object, rule->property_id);
	if (!property || property->dev != crtc->dev ||
	    (property->flags & DRM_MODE_PROP_IMMUTABLE) ||
	    !drm_property_type_is(property, rule->type))
		return -EINVAL;
	/* No request-only sentinels, object IDs, blobs or driver-private language. */
	if (!scene_property(crtc, object, property))
		return -EOPNOTSUPP;
	switch (rule->type) {
	case DRM_MODE_PROP_RANGE:
		if (property->num_values != 2 || rule->minimum < property->values[0] ||
		    rule->maximum > property->values[1])
			return -EINVAL;
		break;
	case DRM_MODE_PROP_SIGNED_RANGE:
		if (property->num_values != 2 || (s64)rule->minimum < (s64)property->values[0] ||
		    (s64)rule->maximum > (s64)property->values[1])
			return -EINVAL;
		break;
	case DRM_MODE_PROP_ENUM:
	case DRM_MODE_PROP_BITMASK:
		for (i = 0; i < property->num_values; i++)
			if (property->values[i] < 64)
				allowed |= BIT_ULL(property->values[i]);
		if (rule->mask & ~allowed)
			return -EINVAL;
		break;
	default:
		return -EOPNOTSUPP;
	}
	return 0;
}

static bool plane_supports_allocation(struct drm_plane *plane,
				     const struct drm_constraints_format *format)
{
	unsigned int i;

	if (!(format->flags & DRM_CONSTRAINTS_FORMAT_IMPLICIT))
		return drm_plane_has_format(plane, format->format, format->modifier);
	/* Implicit layout has no advertised modifier; complete-state checks still apply. */
	for (i = 0; i < plane->format_count; i++)
		if (plane->format_types[i] == format->format)
			return true;
	return false;
}

static int validate_scope(struct drm_crtc *crtc, struct drm_constraints_entry *entry)
{
	struct drm_constraints_description *description;
	const struct drm_constraints_format *formats;
	const struct drm_constraints_property *properties;
	struct drm_plane *plane;
	unsigned int count, i;
	bool found;
	int ret;

	if (!entry || !drm_constraints_entry_in_domain(entry, crtc->dev->mode_config.constraints_domain) ||
	    drm_constraints_entry_crtc(entry) != crtc->base.id)
		return -EINVAL;
	description = drm_constraints_entry_description(entry);
	formats = drm_constraints_description_formats(description, &count);
	for (i = 0; i < count; i++) {
		found = false;
		drm_for_each_plane(plane, crtc->dev) {
			if (plane->base.id == formats[i].plane_id &&
			    (plane->possible_crtcs & drm_crtc_mask(crtc)) &&
			    plane_supports_allocation(plane, &formats[i])) {
				found = true;
				break;
			}
		}
		if (!found)
			return -EINVAL;
	}
	properties = drm_constraints_description_properties(description, &count);
	for (i = 0; i < count; i++) {
		ret = validate_property(crtc, &properties[i]);
		if (ret)
			return ret;
	}
	return 0;
}

int drm_constraints_crtc_init(struct drm_crtc *crtc, struct drm_constraints_entry *initial,
			      unsigned int limit, const struct drm_constraints_output_ops *ops)
{
	struct drm_constraints_output *output;
	int ret;

	if (!crtc->dev->mode_config.constraints_domain || !ops || !ops->check)
		return -EINVAL;
	if (crtc->dev->registered || crtc->constraints_output ||
	    (crtc->state && (crtc->state->enable || crtc->state->active ||
			     crtc->state->commit || crtc->state->constraints)))
		return -EBUSY;
	ret = validate_scope(crtc, initial);
	if (ret)
		return ret;
	output = kzalloc_obj(*output);
	if (!output)
		return -ENOMEM;
	output->list = drm_constraints_list_create(crtc->dev->mode_config.constraints_domain,
							 initial, limit);
	if (IS_ERR(output->list)) {
		ret = PTR_ERR(output->list);
		kfree(output);
		return ret;
	}
	output->ops = ops;
	output->default_entry = drm_constraints_entry_get(initial);
	crtc->constraints_output = output;
	if (crtc->state)
		crtc->state->constraints = drm_constraints_entry_get(initial);
	return 0;
}
EXPORT_SYMBOL_GPL(drm_constraints_crtc_init);

void drm_constraints_crtc_fini(struct drm_crtc *crtc)
{
	struct drm_constraints_output *output = crtc->constraints_output;

	if (!output)
		return;
	crtc->constraints_output = NULL;
	drm_constraints_list_close(output->list);
	drm_constraints_list_put(output->list);
	drm_constraints_entry_put(output->default_entry);
	kfree(output);
}

struct drm_constraints_entry *drm_constraints_crtc_default(struct drm_crtc *crtc)
{
	return crtc->constraints_output ? crtc->constraints_output->default_entry : NULL;
}
EXPORT_SYMBOL_GPL(drm_constraints_crtc_default);

struct drm_constraints_list *drm_constraints_crtc_list(struct drm_crtc *crtc)
{
	return crtc->constraints_output ? crtc->constraints_output->list : NULL;
}
EXPORT_SYMBOL_GPL(drm_constraints_crtc_list);

int drm_constraints_crtc_add(struct drm_crtc *crtc, struct drm_constraints_entry *entry)
{
	int ret;

	if (!crtc->constraints_output)
		return -EOPNOTSUPP;
	ret = validate_scope(crtc, entry);
	if (ret)
		return ret;
	return drm_constraints_list_add(crtc->constraints_output->list, entry);
}
EXPORT_SYMBOL_GPL(drm_constraints_crtc_add);

void drm_constraints_crtc_state_init(struct drm_crtc_state *state)
{
	if (state->crtc->constraints_output)
		state->constraints = drm_constraints_list_selected(state->crtc->constraints_output->list);
}
EXPORT_SYMBOL_GPL(drm_constraints_crtc_state_init);
