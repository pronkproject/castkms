// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/err.h>
#include <linux/export.h>
#include <linux/kref.h>
#include <linux/log2.h>
#include <linux/slab.h>

#include <drm/drm_constraints.h>
#include <drm/drm_fourcc.h>
#include <drm/drm_mode.h>

struct drm_constraints_description {
	struct kref ref;
	struct drm_constraints_size output;
	unsigned int count;
	unsigned int property_count;
	struct drm_constraints_property *properties;
	unsigned int plane_limit_count;
	struct drm_constraints_plane_limit *plane_limits;
	unsigned int plane_geometry_count;
	struct drm_constraints_plane_geometry *plane_geometries;
	struct drm_constraints_format formats[];
};

static int property_valid(const struct drm_constraints_property *property)
{
	if (!property->object_id || !property->property_id)
		return -EINVAL;
	switch (property->type) {
	case DRM_MODE_PROP_RANGE:
		return !property->mask && property->minimum <= property->maximum ? 0 : -EINVAL;
	case DRM_MODE_PROP_SIGNED_RANGE:
		return !property->mask && (s64)property->minimum <= (s64)property->maximum ?
			0 : -EINVAL;
	case DRM_MODE_PROP_ENUM:
		if (!property->mask)
			return -EINVAL;
		fallthrough;
	case DRM_MODE_PROP_BITMASK:
		return !property->minimum && !property->maximum ? 0 : -EINVAL;
	default:
		return -EOPNOTSUPP;
	}
}

bool drm_constraints_property_matches(const struct drm_constraints_property *property, u64 value)
{
	switch (property->type) {
	case DRM_MODE_PROP_RANGE:
		return value >= property->minimum && value <= property->maximum;
	case DRM_MODE_PROP_SIGNED_RANGE:
		return (s64)value >= (s64)property->minimum && (s64)value <= (s64)property->maximum;
	case DRM_MODE_PROP_ENUM:
		return value < 64 && (property->mask & BIT_ULL(value));
	case DRM_MODE_PROP_BITMASK:
		return !(value & ~property->mask);
	default:
		return false;
	}
}
EXPORT_SYMBOL_GPL(drm_constraints_property_matches);

static bool size_valid(const struct drm_constraints_size *size)
{
	return size->min_width && size->min_height &&
		size->min_width <= size->max_width &&
		size->min_height <= size->max_height;
}

struct drm_constraints_description *
drm_constraints_description_create_with_geometry(
	const struct drm_constraints_size *output,
	const struct drm_constraints_format *formats,
	unsigned int count,
	const struct drm_constraints_property *properties,
	unsigned int property_count,
	const struct drm_constraints_plane_limit *plane_limits,
	unsigned int plane_limit_count,
	const struct drm_constraints_plane_geometry *plane_geometries,
	unsigned int plane_geometry_count)
{
	struct drm_constraints_description *description;
	unsigned int i, j, copied_limits = 0;
	int ret;

	if (!output || !formats || !count || count > DRM_CONSTRAINTS_MAX_FORMATS ||
	    !size_valid(output) || property_count > DRM_CONSTRAINTS_MAX_PROPERTIES ||
	    (property_count && !properties) ||
	    plane_limit_count > DRM_CONSTRAINTS_MAX_PLANE_LIMITS ||
	    (plane_limit_count && !plane_limits) ||
	    plane_geometry_count > DRM_CONSTRAINTS_MAX_PLANE_GEOMETRIES ||
	    (plane_geometry_count && !plane_geometries))
		return ERR_PTR(-EINVAL);
	for (i = 0; i < count; i++) {
		if (!formats[i].plane_id || !__drm_format_info(formats[i].format) ||
		    formats[i].modifier == DRM_FORMAT_MOD_INVALID ||
		    (formats[i].flags & ~DRM_CONSTRAINTS_FORMAT_IMPLICIT) ||
		    (formats[i].flags && formats[i].modifier) ||
		    !formats[i].storage_flags ||
		    (formats[i].storage_flags & ~(DRM_CONSTRAINTS_FORMAT_STORAGE_NATIVE |
						 DRM_CONSTRAINTS_FORMAT_STORAGE_IMPORTED)) ||
		    !is_power_of_2(formats[i].pitch_alignment) ||
		    !is_power_of_2(formats[i].offset_alignment) ||
		    formats[i].max_pitch < formats[i].pitch_alignment ||
		    !size_valid(&formats[i].size))
			return ERR_PTR(-EINVAL);
		for (j = 0; j < i; j++) {
			if (formats[i].plane_id == formats[j].plane_id &&
			    formats[i].format == formats[j].format &&
			    formats[i].modifier == formats[j].modifier &&
			    formats[i].flags == formats[j].flags)
				return ERR_PTR(-EEXIST);
		}
	}
	for (i = 0; i < property_count; i++) {
		ret = property_valid(&properties[i]);
		if (ret)
			return ERR_PTR(ret);
		for (j = 0; j < i; j++)
			if (properties[i].object_id == properties[j].object_id &&
			    properties[i].property_id == properties[j].property_id)
				return ERR_PTR(-EEXIST);
	}
	for (i = 0; i < plane_limit_count; i++) {
		if (!plane_limits[i].max_active || !plane_limits[i].count ||
		    plane_limits[i].max_active > plane_limits[i].count ||
		    plane_limits[i].count > DRM_CONSTRAINTS_MAX_PLANES_PER_LIMIT ||
		    !plane_limits[i].plane_ids)
			return ERR_PTR(-EINVAL);
		for (j = 0; j < plane_limits[i].count; j++) {
			unsigned int previous;

			if (!plane_limits[i].plane_ids[j])
				return ERR_PTR(-EINVAL);
			for (previous = 0; previous < j; previous++)
				if (plane_limits[i].plane_ids[j] ==
				    plane_limits[i].plane_ids[previous])
					return ERR_PTR(-EEXIST);
		}
	}
	for (i = 0; i < plane_geometry_count; i++) {
		if (!plane_geometries[i].plane_id ||
		    (plane_geometries[i].flags & ~DRM_CONSTRAINTS_GEOMETRY_FLAGS) ||
		    !plane_geometries[i].min_scale ||
		    plane_geometries[i].min_scale > plane_geometries[i].max_scale)
			return ERR_PTR(-EINVAL);
		for (j = 0; j < i; j++)
			if (plane_geometries[i].plane_id == plane_geometries[j].plane_id)
				return ERR_PTR(-EEXIST);
	}
	description = kvzalloc(struct_size(description, formats, count), GFP_KERNEL);
	if (!description)
		return ERR_PTR(-ENOMEM);
	if (property_count) {
		description->properties = kmemdup(properties, sizeof(*properties) * property_count,
						 GFP_KERNEL);
		if (!description->properties) {
			kvfree(description);
			return ERR_PTR(-ENOMEM);
		}
	}
	if (plane_limit_count) {
		description->plane_limits = kcalloc(plane_limit_count,
						    sizeof(*description->plane_limits), GFP_KERNEL);
		if (!description->plane_limits)
			goto err_properties;
		for (i = 0; i < plane_limit_count; i++) {
			description->plane_limits[i] = plane_limits[i];
			description->plane_limits[i].plane_ids =
				kmemdup_array(plane_limits[i].plane_ids, plane_limits[i].count,
					      sizeof(*plane_limits[i].plane_ids), GFP_KERNEL);
			if (!description->plane_limits[i].plane_ids)
				goto err_plane_limits;
			copied_limits++;
		}
	}
	if (plane_geometry_count) {
		description->plane_geometries = kmemdup_array(plane_geometries,
							      plane_geometry_count,
							      sizeof(*plane_geometries),
							      GFP_KERNEL);
		if (!description->plane_geometries)
			goto err_plane_limits;
	}
	kref_init(&description->ref);
	description->output = *output;
	description->count = count;
	description->property_count = property_count;
	description->plane_limit_count = plane_limit_count;
	description->plane_geometry_count = plane_geometry_count;
	memcpy(description->formats, formats, sizeof(*formats) * count);
	return description;

err_plane_limits:
	while (copied_limits)
		kfree(description->plane_limits[--copied_limits].plane_ids);
	kfree(description->plane_limits);
err_properties:
	kfree(description->properties);
	kvfree(description);
	return ERR_PTR(-ENOMEM);
}
EXPORT_SYMBOL_GPL(drm_constraints_description_create_with_geometry);

struct drm_constraints_description *
drm_constraints_description_create(const struct drm_constraints_size *output,
				   const struct drm_constraints_format *formats,
				   unsigned int count,
				   const struct drm_constraints_property *properties,
				   unsigned int property_count,
				   const struct drm_constraints_plane_limit *plane_limits,
				   unsigned int plane_limit_count)
{
	return drm_constraints_description_create_with_geometry(output, formats, count,
			properties, property_count, plane_limits, plane_limit_count, NULL, 0);
}
EXPORT_SYMBOL_GPL(drm_constraints_description_create);

struct drm_constraints_description *
drm_constraints_description_get(struct drm_constraints_description *description)
{
	kref_get(&description->ref);
	return description;
}
EXPORT_SYMBOL_GPL(drm_constraints_description_get);

static void description_free(struct kref *ref)
{
	struct drm_constraints_description *description =
		container_of(ref, struct drm_constraints_description, ref);
	unsigned int i;

	for (i = 0; i < description->plane_limit_count; i++)
		kfree(description->plane_limits[i].plane_ids);
	kfree(description->plane_geometries);
	kfree(description->plane_limits);
	kfree(description->properties);
	kvfree(description);
}

void drm_constraints_description_put(struct drm_constraints_description *description)
{
	kref_put(&description->ref, description_free);
}
EXPORT_SYMBOL_GPL(drm_constraints_description_put);

const struct drm_constraints_size *
drm_constraints_description_output(const struct drm_constraints_description *description)
{
	return &description->output;
}
EXPORT_SYMBOL_GPL(drm_constraints_description_output);

const struct drm_constraints_format *
drm_constraints_description_formats(const struct drm_constraints_description *description,
				    unsigned int *count)
{
	*count = description->count;
	return description->formats;
}
EXPORT_SYMBOL_GPL(drm_constraints_description_formats);

const struct drm_constraints_property *
drm_constraints_description_properties(const struct drm_constraints_description *description,
				       unsigned int *count)
{
	*count = description->property_count;
	return description->properties;
}
EXPORT_SYMBOL_GPL(drm_constraints_description_properties);

const struct drm_constraints_plane_limit *
drm_constraints_description_plane_limits(const struct drm_constraints_description *description,
					 unsigned int *count)
{
	*count = description->plane_limit_count;
	return description->plane_limits;
}
EXPORT_SYMBOL_GPL(drm_constraints_description_plane_limits);

const struct drm_constraints_plane_geometry *
drm_constraints_description_plane_geometries(const struct drm_constraints_description *description,
					     unsigned int *count)
{
	*count = description->plane_geometry_count;
	return description->plane_geometries;
}
EXPORT_SYMBOL_GPL(drm_constraints_description_plane_geometries);

static bool size_covers(const struct drm_constraints_size *candidate,
			const struct drm_constraints_size *required)
{
	return candidate->min_width <= required->min_width &&
		candidate->min_height <= required->min_height &&
		candidate->max_width >= required->max_width &&
		candidate->max_height >= required->max_height;
}

static bool format_covers(const struct drm_constraints_format *candidate,
			  const struct drm_constraints_format *required)
{
	return candidate->plane_id == required->plane_id &&
		candidate->format == required->format &&
		candidate->modifier == required->modifier &&
		candidate->flags == required->flags &&
		size_covers(&candidate->size, &required->size) &&
		(candidate->storage_flags & required->storage_flags) == required->storage_flags &&
		!(required->pitch_alignment % candidate->pitch_alignment) &&
		!(required->offset_alignment % candidate->offset_alignment) &&
		candidate->max_pitch >= required->max_pitch;
}

static bool property_covers(const struct drm_constraints_property *candidate,
			    const struct drm_constraints_property *required)
{
	if (candidate->object_id != required->object_id ||
	    candidate->property_id != required->property_id ||
	    candidate->type != required->type)
		return false;

	switch (candidate->type) {
	case DRM_MODE_PROP_RANGE:
		return candidate->minimum <= required->minimum &&
			candidate->maximum >= required->maximum;
	case DRM_MODE_PROP_SIGNED_RANGE:
		return (s64)candidate->minimum <= (s64)required->minimum &&
			(s64)candidate->maximum >= (s64)required->maximum;
	case DRM_MODE_PROP_ENUM:
	case DRM_MODE_PROP_BITMASK:
		return !(required->mask & ~candidate->mask);
	default:
		return false;
	}
}

static bool plane_sets_equal(const struct drm_constraints_plane_limit *left,
			     const struct drm_constraints_plane_limit *right)
{
	unsigned int i, j;

	if (left->count != right->count)
		return false;
	for (i = 0; i < left->count; i++) {
		for (j = 0; j < right->count; j++)
			if (left->plane_ids[i] == right->plane_ids[j])
				break;
		if (j == right->count)
			return false;
	}
	return true;
}

static bool geometry_covers(const struct drm_constraints_plane_geometry *candidate,
			    const struct drm_constraints_plane_geometry *required)
{
	return candidate->plane_id == required->plane_id &&
		(candidate->flags & required->flags) == required->flags &&
		candidate->min_scale <= required->min_scale &&
		candidate->max_scale >= required->max_scale;
}

bool drm_constraints_description_covers(const struct drm_constraints_description *candidate,
					const struct drm_constraints_description *required)
{
	unsigned int i, j;

	if (!candidate || !required || !size_covers(&candidate->output, &required->output))
		return false;

	for (i = 0; i < required->count; i++) {
		for (j = 0; j < candidate->count; j++)
			if (format_covers(&candidate->formats[j], &required->formats[i]))
				break;
		if (j == candidate->count)
			return false;
	}

	/* A missing candidate rule leaves the ordinary property domain unrestricted. */
	for (i = 0; i < candidate->property_count; i++) {
		for (j = 0; j < required->property_count; j++)
			if (property_covers(&candidate->properties[i], &required->properties[j]))
				break;
		if (j == required->property_count)
			return false;
	}

	for (i = 0; i < candidate->plane_limit_count; i++) {
		const struct drm_constraints_plane_limit *limit = &candidate->plane_limits[i];

		if (limit->max_active == limit->count)
			continue;
		for (j = 0; j < required->plane_limit_count; j++) {
			const struct drm_constraints_plane_limit *required_limit =
				&required->plane_limits[j];

			if (limit->max_active >= required_limit->max_active &&
			    plane_sets_equal(limit, required_limit))
				break;
		}
		if (j == required->plane_limit_count)
			return false;
	}

	/* A missing candidate rule leaves ordinary plane geometry unrestricted. */
	for (i = 0; i < candidate->plane_geometry_count; i++) {
		for (j = 0; j < required->plane_geometry_count; j++)
			if (geometry_covers(&candidate->plane_geometries[i],
					    &required->plane_geometries[j]))
				break;
		if (j == required->plane_geometry_count)
			return false;
	}
	return true;
}
EXPORT_SYMBOL_GPL(drm_constraints_description_covers);
