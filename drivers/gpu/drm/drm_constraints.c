// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/err.h>
#include <linux/export.h>
#include <linux/kref.h>
#include <linux/slab.h>

#include <drm/drm_constraints.h>
#include <drm/drm_fourcc.h>

struct drm_constraints_description {
	struct kref ref;
	struct drm_constraints_size output;
	unsigned int count;
	struct drm_constraints_format formats[];
};

static bool size_valid(const struct drm_constraints_size *size)
{
	return size->min_width && size->min_height &&
		size->min_width <= size->max_width &&
		size->min_height <= size->max_height;
}

struct drm_constraints_description *
drm_constraints_description_create(const struct drm_constraints_size *output,
				   const struct drm_constraints_format *formats,
				   unsigned int count)
{
	struct drm_constraints_description *description;
	unsigned int i, j;

	if (!output || !formats || !count || count > DRM_CONSTRAINTS_MAX_FORMATS ||
	    !size_valid(output))
		return ERR_PTR(-EINVAL);
	for (i = 0; i < count; i++) {
		if (!formats[i].plane_id || !__drm_format_info(formats[i].format) ||
		    formats[i].modifier == DRM_FORMAT_MOD_INVALID ||
		    !size_valid(&formats[i].size))
			return ERR_PTR(-EINVAL);
		for (j = 0; j < i; j++) {
			if (formats[i].plane_id == formats[j].plane_id &&
			    formats[i].format == formats[j].format &&
			    formats[i].modifier == formats[j].modifier)
				return ERR_PTR(-EEXIST);
		}
	}
	description = kzalloc(struct_size(description, formats, count), GFP_KERNEL);
	if (!description)
		return ERR_PTR(-ENOMEM);
	kref_init(&description->ref);
	description->output = *output;
	description->count = count;
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
	kfree(container_of(ref, struct drm_constraints_description, ref));
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
