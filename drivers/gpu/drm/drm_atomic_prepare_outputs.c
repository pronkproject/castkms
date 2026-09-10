// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/err.h>
#include <linux/export.h>
#include <linux/slab.h>
#include <drm/drm_atomic_prepare.h>
#include <drm/drm_atomic_prepare_outputs.h>

#include "drm_atomic_prepare_internal.h"

struct drm_prepare_outputs {
	unsigned int count;
	struct drm_prepare_output_generation entries[] __counted_by(count);
};

static int validate_entries(const struct drm_prepare_output_generation *entries,
			    unsigned int count)
{
	unsigned int i, j;

	if (count > DRM_PREPARE_MAX_OUTPUTS)
		return -E2BIG;
	if (count && !entries)
		return -EINVAL;
	for (i = 0; i < count; i++) {
		if (!entries[i].crtc_id || !entries[i].source)
			return -EINVAL;
		for (j = 0; j < i; j++) {
			if (entries[j].crtc_id == entries[i].crtc_id)
				return -EINVAL;
		}
	}
	return 0;
}

struct drm_prepare_outputs *
drm_prepare_outputs_create(const struct drm_prepare_output_generation *entries,
			 unsigned int count)
{
	struct drm_prepare_outputs *outputs;
	unsigned int i;
	int ret;

	ret = validate_entries(entries, count);
	if (ret)
		return ERR_PTR(ret);
	for (i = 1; i < count; i++) {
		if (drm_prepare_source_domain(entries[i].source) !=
		    drm_prepare_source_domain(entries[0].source))
			return ERR_PTR(-EXDEV);
	}
	outputs = kmalloc(struct_size(outputs, entries, count), GFP_KERNEL);
	if (!outputs)
		return ERR_PTR(-ENOMEM);
	outputs->count = count;
	for (i = 0; i < count; i++) {
		outputs->entries[i].crtc_id = entries[i].crtc_id;
		outputs->entries[i].source = drm_prepare_source_get(entries[i].source);
	}
	return outputs;
}
EXPORT_SYMBOL_GPL(drm_prepare_outputs_create);

void drm_prepare_outputs_destroy(struct drm_prepare_outputs *outputs)
{
	unsigned int i;

	for (i = 0; i < outputs->count; i++)
		drm_prepare_source_put(outputs->entries[i].source);
	kfree(outputs);
}
EXPORT_SYMBOL_GPL(drm_prepare_outputs_destroy);

int drm_prepare_outputs_validate(const struct drm_prepare_outputs *outputs,
			       const struct drm_prepare_output_generation *observed,
			       unsigned int count)
{
	unsigned int i, j;
	int ret;

	ret = validate_entries(observed, count);
	if (ret)
		return ret;
	if (outputs->count != count)
		return -ESTALE;
	for (i = 0; i < count; i++) {
		for (j = 0; j < count; j++) {
			if (outputs->entries[i].crtc_id == observed[j].crtc_id)
				break;
		}
		if (j == count || outputs->entries[i].source != observed[j].source)
			return -ESTALE;
	}
	return 0;
}
EXPORT_SYMBOL_GPL(drm_prepare_outputs_validate);

struct drm_prepare_retirement_set *
drm_prepare_outputs_hold(const struct drm_prepare_outputs *outputs)
{
	struct drm_prepare_source *sources[DRM_PREPARE_MAX_OUTPUTS];
	unsigned int i;

	for (i = 0; i < outputs->count; i++)
		sources[i] = outputs->entries[i].source;
	return drm_prepare_retirement_set_create(sources, outputs->count);
}
EXPORT_SYMBOL_GPL(drm_prepare_outputs_hold);
