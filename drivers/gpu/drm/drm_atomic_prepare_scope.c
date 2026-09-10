// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/err.h>
#include <linux/export.h>
#include <linux/slab.h>
#include <drm/drm_atomic_prepare.h>
#include <drm/drm_atomic_prepare_scope.h>

#include "drm_atomic_prepare_internal.h"

struct drm_prepare_scope {
	unsigned int count;
	struct drm_prepare_scope_entry entries[] __counted_by(count);
};

static int validate_entries(const struct drm_prepare_scope_entry *entries,
			    unsigned int count)
{
	unsigned int i, j;

	if (count > DRM_PREPARE_SCOPE_MAX_OUTPUTS)
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

struct drm_prepare_scope *
drm_prepare_scope_create(const struct drm_prepare_scope_entry *entries,
			 unsigned int count)
{
	struct drm_prepare_scope *scope;
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
	scope = kmalloc(struct_size(scope, entries, count), GFP_KERNEL);
	if (!scope)
		return ERR_PTR(-ENOMEM);
	scope->count = count;
	for (i = 0; i < count; i++) {
		scope->entries[i].crtc_id = entries[i].crtc_id;
		scope->entries[i].source = drm_prepare_source_get(entries[i].source);
	}
	return scope;
}
EXPORT_SYMBOL_GPL(drm_prepare_scope_create);

void drm_prepare_scope_destroy(struct drm_prepare_scope *scope)
{
	unsigned int i;

	for (i = 0; i < scope->count; i++)
		drm_prepare_source_put(scope->entries[i].source);
	kfree(scope);
}
EXPORT_SYMBOL_GPL(drm_prepare_scope_destroy);

int drm_prepare_scope_validate(const struct drm_prepare_scope *scope,
			       const struct drm_prepare_scope_entry *observed,
			       unsigned int count)
{
	unsigned int i, j;
	int ret;

	ret = validate_entries(observed, count);
	if (ret)
		return ret;
	if (scope->count != count)
		return -ESTALE;
	for (i = 0; i < count; i++) {
		for (j = 0; j < count; j++) {
			if (scope->entries[i].crtc_id == observed[j].crtc_id)
				break;
		}
		if (j == count || scope->entries[i].source != observed[j].source)
			return -ESTALE;
	}
	return 0;
}
EXPORT_SYMBOL_GPL(drm_prepare_scope_validate);

struct drm_prepare_retirement_set *
drm_prepare_scope_hold(const struct drm_prepare_scope *scope)
{
	struct drm_prepare_source *sources[DRM_PREPARE_SCOPE_MAX_OUTPUTS];
	unsigned int i;

	for (i = 0; i < scope->count; i++)
		sources[i] = scope->entries[i].source;
	return drm_prepare_retirement_set_create(sources, scope->count);
}
EXPORT_SYMBOL_GPL(drm_prepare_scope_hold);
