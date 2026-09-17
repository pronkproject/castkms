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
drm_constraints_description_create(const struct drm_constraints_size *output,
				   const struct drm_constraints_format *formats,
				   unsigned int count,
				   const struct drm_constraints_property *properties,
				   unsigned int property_count)
{
	struct drm_constraints_description *description;
	unsigned int i, j;
	int ret;

	if (!output || !formats || !count || count > DRM_CONSTRAINTS_MAX_FORMATS ||
	    !size_valid(output) || property_count > DRM_CONSTRAINTS_MAX_PROPERTIES ||
	    (property_count && !properties))
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
	kref_init(&description->ref);
	description->output = *output;
	description->count = count;
	description->property_count = property_count;
	memcpy(description->formats, formats, sizeof(*formats) * count);
	return description;
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
