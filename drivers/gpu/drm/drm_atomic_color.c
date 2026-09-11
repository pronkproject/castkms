// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/export.h>
#include <linux/overflow.h>
#include <drm/drm_atomic_uapi.h>
#include <drm/drm_crtc.h>
#include <drm/drm_device.h>
#include <drm/drm_property.h>

#include "drm_crtc_internal.h"

bool drm_atomic_is_crtc_color_property(struct drm_crtc *crtc,
				       struct drm_property *property)
{
	struct drm_mode_config *config = &crtc->dev->mode_config;

	return property && (property == config->degamma_lut_property ||
			    property == config->ctm_property ||
			    property == config->gamma_lut_property);
}

/**
 * drm_atomic_set_color_property_for_crtc - set a resolved controller color blob
 * @state: uncommitted controller state protected by its modeset lock
 * @property: attached DEGAMMA_LUT, CTM or GAMMA_LUT property
 * @blob: caller-owned blob, or NULL to clear the property
 *
 * Checks attachment, device membership and the property's size constraints.
 * Lookup tables contain whole color entries and cannot exceed the advertised
 * table size. A matrix has the exact size of struct drm_color_ctm. The pending
 * state takes its own reference and records a color change only on replacement.
 * Driver-specific color constraints remain part of the complete atomic check.
 * No identifiers are looked up and the caller's blob reference is not consumed.
 *
 * Return: 0 on success, -EOPNOTSUPP for another property, -EINVAL for invalid
 * attachment or blob, or a negative error reading the advertised table size.
 * An unrepresentable table size returns -EOVERFLOW. Errors preserve @state.
 */
int drm_atomic_set_color_property_for_crtc(struct drm_crtc_state *state,
					  struct drm_property *property,
					  struct drm_property_blob *blob)
{
	struct drm_crtc *crtc = state->crtc;
	struct drm_device *dev = crtc->dev;
	struct drm_mode_config *config = &dev->mode_config;
	struct drm_property *size_property;
	struct drm_property_blob **destination;
	ssize_t max_size = -1, exact_size = -1, elem_size = -1;
	bool replaced = false;
	int ret;

	if (!drm_atomic_is_crtc_color_property(crtc, property))
		return -EOPNOTSUPP;
	if (drm_mode_obj_find_prop_id(&crtc->base, property->base.id) != property)
		return -EINVAL;
	if (property == config->ctm_property) {
		destination = &state->ctm;
		exact_size = sizeof(struct drm_color_ctm);
	} else {
		u64 count, bytes;

		if (property == config->degamma_lut_property) {
			destination = &state->degamma_lut;
			size_property = config->degamma_lut_size_property;
		} else {
			destination = &state->gamma_lut;
			size_property = config->gamma_lut_size_property;
		}
		ret = drm_object_immutable_property_get_value(&crtc->base, size_property, &count);
		if (ret)
			return ret;
		elem_size = sizeof(struct drm_color_lut);
		if (check_mul_overflow(count, (u64)elem_size, &bytes) || bytes > SSIZE_MAX)
			return -EOVERFLOW;
		if (!bytes && blob)
			return -EINVAL;
		max_size = bytes;
	}
	ret = drm_property_replace_blob_checked(dev, destination, blob, max_size,
						exact_size, elem_size, &replaced);
	state->color_mgmt_changed |= replaced;
	return ret;
}
EXPORT_SYMBOL_GPL(drm_atomic_set_color_property_for_crtc);
