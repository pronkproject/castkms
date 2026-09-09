// SPDX-License-Identifier: GPL-2.0 OR MIT
/* Fixed ownership of admission holds, independent of ticket transport. */

#include <linux/err.h>
#include <linux/export.h>
#include <linux/kref.h>
#include <linux/slab.h>
#include <linux/sort.h>

#include <drm/drm_atomic_prepare.h>

#include "drm_atomic_prepare_internal.h"

struct drm_prepare_retirement_set {
	struct kref ref;
	unsigned int count;
	struct drm_prepare_admission_hold *holds[] __counted_by(count);
};

static int compare_sources(const void *a, const void *b)
{
	unsigned long left = (unsigned long)*(struct drm_prepare_source * const *)a;
	unsigned long right = (unsigned long)*(struct drm_prepare_source * const *)b;

	return (left > right) - (left < right);
}

struct drm_prepare_retirement_set *
drm_prepare_retirement_set_create(struct drm_prepare_source * const *sources,
				  unsigned int count)
{
	struct drm_prepare_retirement_set *set;
	struct drm_prepare_source **sorted;
	unsigned int i, unique = 0;
	int error;

	if (count && !sources)
		return ERR_PTR(-EINVAL);
	for (i = 0; i < count; i++) {
		if (!sources[i])
			return ERR_PTR(-EINVAL);
	}
	sorted = kmalloc_array(count, sizeof(*sorted), GFP_KERNEL);
	if (!sorted)
		return ERR_PTR(-ENOMEM);
	for (i = 0; i < count; i++)
		sorted[i] = sources[i];
	sort(sorted, count, sizeof(*sorted), compare_sources, NULL);
	for (i = 0; i < count; i++) {
		if (!unique || sorted[i] != sorted[unique - 1])
			sorted[unique++] = sorted[i];
	}
	set = kzalloc(struct_size(set, holds, unique), GFP_KERNEL);
	if (!set) {
		error = -ENOMEM;
		goto free_sorted;
	}
	set->count = unique;
	error = drm_prepare_hold_sources(sorted, set->holds, unique);
	if (error) {
		kfree(set);
		goto free_sorted;
	}
	kref_init(&set->ref);
	kfree(sorted);
	return set;

free_sorted:
	kfree(sorted);
	return ERR_PTR(error);
}
EXPORT_SYMBOL_GPL(drm_prepare_retirement_set_create);

struct drm_prepare_retirement_set *
drm_prepare_retirement_set_get(struct drm_prepare_retirement_set *set)
{
	kref_get(&set->ref);
	return set;
}
EXPORT_SYMBOL_GPL(drm_prepare_retirement_set_get);

static void retirement_set_free(struct kref *ref)
{
	struct drm_prepare_retirement_set *set;
	unsigned int i;

	set = container_of(ref, struct drm_prepare_retirement_set, ref);
	for (i = 0; i < set->count; i++)
		drm_prepare_admission_hold_put(set->holds[i]);
	kfree(set);
}

void drm_prepare_retirement_set_put(struct drm_prepare_retirement_set *set)
{
	kref_put(&set->ref, retirement_set_free);
}
EXPORT_SYMBOL_GPL(drm_prepare_retirement_set_put);
