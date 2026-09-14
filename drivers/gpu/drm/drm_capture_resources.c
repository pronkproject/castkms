// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/err.h>
#include <linux/export.h>
#include <linux/limits.h>
#include <linux/slab.h>

#include <drm/drm_capture_resources.h>

struct drm_capture_resources {
	u64 last_id;
	u32 limit;
	u64 ids[];
};

struct drm_capture_resources *drm_capture_resources_create(u32 limit)
{
	struct drm_capture_resources *resources;

	if (!limit || limit > INT_MAX)
		return ERR_PTR(-EINVAL);
	resources = kzalloc(struct_size(resources, ids, limit), GFP_KERNEL);
	if (!resources)
		return ERR_PTR(-ENOMEM);
	resources->limit = limit;
	return resources;
}
EXPORT_SYMBOL_GPL(drm_capture_resources_create);

void drm_capture_resources_destroy(struct drm_capture_resources *resources)
{
	kfree(resources);
}
EXPORT_SYMBOL_GPL(drm_capture_resources_destroy);

int drm_capture_resources_check(const struct drm_capture_resources *resources, u64 id)
{
	u32 i;

	if (!id)
		return -EINVAL;
	if (resources->last_id == U64_MAX)
		return -EOVERFLOW;
	if (id <= resources->last_id)
		return -ESTALE;
	for (i = 0; i < resources->limit; i++)
		if (!resources->ids[i])
			return i;
	return -EBUSY;
}
EXPORT_SYMBOL_GPL(drm_capture_resources_check);

int drm_capture_resources_insert(struct drm_capture_resources *resources, u64 id)
{
	int slot = drm_capture_resources_check(resources, id);

	if (slot < 0)
		return slot;
	resources->ids[slot] = id;
	resources->last_id = id;
	return slot;
}
EXPORT_SYMBOL_GPL(drm_capture_resources_insert);

int drm_capture_resources_find(const struct drm_capture_resources *resources, u64 id)
{
	u32 i;

	if (!id)
		return -EINVAL;
	for (i = 0; i < resources->limit; i++)
		if (resources->ids[i] == id)
			return i;
	return -ENOENT;
}
EXPORT_SYMBOL_GPL(drm_capture_resources_find);

int drm_capture_resources_remove(struct drm_capture_resources *resources, u64 id)
{
	int slot = drm_capture_resources_find(resources, id);

	if (slot >= 0)
		resources->ids[slot] = 0;
	return slot;
}
EXPORT_SYMBOL_GPL(drm_capture_resources_remove);
