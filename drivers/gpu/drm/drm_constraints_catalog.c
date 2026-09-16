// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/err.h>
#include <linux/kref.h>
#include <linux/module.h>
#include <linux/mutex.h>
#include <linux/slab.h>

#include <drm/drm_constraints_catalog.h>
#include <drm/drm_constraints_entry.h>

#include "drm_constraints_internal.h"

struct drm_constraints_catalog {
	struct kref ref;
	struct mutex lock;
	struct drm_constraints_domain *domain;
	u32 crtc_id;
	unsigned int limit;
	bool closed;
	struct drm_constraints_snapshot_info info;
	struct drm_constraints_listing entries[];
};

struct drm_constraints_snapshot {
	struct drm_constraints_snapshot_info info;
	struct drm_constraints_listing entries[];
};

struct drm_constraints_catalog *
drm_constraints_catalog_create(struct drm_constraints_domain *domain,
			       struct drm_constraints_entry *initial, unsigned int limit)
{
	struct drm_constraints_catalog *catalog;

	if (!domain || !initial || !limit || limit > DRM_CONSTRAINTS_MAX_ENTRIES ||
	    !drm_constraints_entry_in_domain(initial, domain))
		return ERR_PTR(-EINVAL);
	catalog = kzalloc(struct_size(catalog, entries, limit), GFP_KERNEL);
	if (!catalog)
		return ERR_PTR(-ENOMEM);
	kref_init(&catalog->ref);
	mutex_init(&catalog->lock);
	catalog->domain = drm_constraints_domain_get(domain);
	catalog->crtc_id = drm_constraints_entry_crtc(initial);
	catalog->limit = limit;
	catalog->info.generation = 1;
	catalog->info.selected_id = drm_constraints_entry_id(initial);
	catalog->info.count = 1;
	catalog->entries[0].entry = drm_constraints_entry_get(initial);
	catalog->entries[0].selectable = true;
	return catalog;
}
EXPORT_SYMBOL_GPL(drm_constraints_catalog_create);

struct drm_constraints_catalog *
drm_constraints_catalog_get(struct drm_constraints_catalog *catalog)
{
	kref_get(&catalog->ref);
	return catalog;
}
EXPORT_SYMBOL_GPL(drm_constraints_catalog_get);

static void catalog_free(struct kref *ref)
{
	struct drm_constraints_catalog *catalog = container_of(ref, struct drm_constraints_catalog, ref);
	unsigned int i;

	for (i = 0; i < catalog->info.count; i++)
		drm_constraints_entry_put(catalog->entries[i].entry);
	drm_constraints_domain_put(catalog->domain);
	mutex_destroy(&catalog->lock);
	kfree(catalog);
}

void drm_constraints_catalog_put(struct drm_constraints_catalog *catalog)
{
	kref_put(&catalog->ref, catalog_free);
}
EXPORT_SYMBOL_GPL(drm_constraints_catalog_put);

void drm_constraints_catalog_close(struct drm_constraints_catalog *catalog)
{
	mutex_lock(&catalog->lock);
	catalog->closed = true;
	mutex_unlock(&catalog->lock);
}
EXPORT_SYMBOL_GPL(drm_constraints_catalog_close);

static int find_entry(struct drm_constraints_catalog *catalog, u64 id)
{
	unsigned int i;

	lockdep_assert_held(&catalog->lock);
	for (i = 0; i < catalog->info.count; i++)
		if (drm_constraints_entry_id(catalog->entries[i].entry) == id)
			return i;
	return -ENOENT;
}

struct drm_constraints_entry *
drm_constraints_catalog_selected(struct drm_constraints_catalog *catalog)
{
	struct drm_constraints_entry *entry;
	int index;

	mutex_lock(&catalog->lock);
	index = find_entry(catalog, catalog->info.selected_id);
	entry = drm_constraints_entry_get(catalog->entries[index].entry);
	mutex_unlock(&catalog->lock);
	return entry;
}
EXPORT_SYMBOL_GPL(drm_constraints_catalog_selected);

int drm_constraints_catalog_quiesce(struct drm_constraints_catalog *catalog,
				    struct drm_constraints_entry *entry,
				    int (*quiesce)(struct drm_constraints_entry *, void *),
				    void *data)
{
	int index, ret;

	mutex_lock(&catalog->lock);
	index = find_entry(catalog, catalog->info.selected_id);
	if (catalog->entries[index].entry != entry)
		ret = -ESTALE;
	else
		ret = quiesce(entry, data);
	mutex_unlock(&catalog->lock);
	return ret > 0 ? -EINVAL : ret;
}

int drm_constraints_catalog_add(struct drm_constraints_catalog *catalog,
				struct drm_constraints_entry *entry)
{
	int ret = 0;

	if (!entry || !drm_constraints_entry_in_domain(entry, catalog->domain) ||
	    drm_constraints_entry_crtc(entry) != catalog->crtc_id)
		return -EINVAL;
	mutex_lock(&catalog->lock);
	if (catalog->closed)
		ret = -ESTALE;
	else if (find_entry(catalog, drm_constraints_entry_id(entry)) >= 0)
		ret = -EEXIST;
	else if (catalog->info.count == catalog->limit)
		ret = -ENOSPC;
	else if (catalog->info.generation == U64_MAX)
		ret = -EOVERFLOW;
	else {
		catalog->entries[catalog->info.count++] = (struct drm_constraints_listing) {
			.entry = drm_constraints_entry_get(entry),
			.selectable = true,
		};
		catalog->info.generation++;
	}
	mutex_unlock(&catalog->lock);
	return ret;
}
EXPORT_SYMBOL_GPL(drm_constraints_catalog_add);

int drm_constraints_catalog_withdraw(struct drm_constraints_catalog *catalog, u64 id)
{
	int index, ret = 0;

	mutex_lock(&catalog->lock);
	index = find_entry(catalog, id);
	if (index < 0)
		ret = index;
	else if (!catalog->entries[index].selectable)
		ret = 0;
	else if (catalog->info.generation == U64_MAX)
		ret = -EOVERFLOW;
	else {
		catalog->entries[index].selectable = false;
		if (catalog->info.suggested_id == id)
			catalog->info.suggested_id = 0;
		catalog->info.generation++;
	}
	mutex_unlock(&catalog->lock);
	return ret;
}
EXPORT_SYMBOL_GPL(drm_constraints_catalog_withdraw);

int drm_constraints_catalog_forget(struct drm_constraints_catalog *catalog, u64 id)
{
	struct drm_constraints_entry *entry = NULL;
	int index, ret = 0;

	mutex_lock(&catalog->lock);
	index = find_entry(catalog, id);
	if (index < 0)
		ret = index;
	else if (catalog->entries[index].selectable || catalog->info.selected_id == id)
		ret = -EBUSY;
	else if (catalog->info.generation == U64_MAX)
		ret = -EOVERFLOW;
	else {
		entry = catalog->entries[index].entry;
		catalog->info.count--;
		memmove(&catalog->entries[index], &catalog->entries[index + 1],
			(catalog->info.count - index) * sizeof(*catalog->entries));
		memset(&catalog->entries[catalog->info.count], 0, sizeof(*catalog->entries));
		catalog->info.generation++;
	}
	mutex_unlock(&catalog->lock);
	if (entry)
		drm_constraints_entry_put(entry);
	return ret;
}
EXPORT_SYMBOL_GPL(drm_constraints_catalog_forget);

int drm_constraints_catalog_suggest(struct drm_constraints_catalog *catalog, u64 id)
{
	int index, ret = 0;

	mutex_lock(&catalog->lock);
	index = find_entry(catalog, id);
	if (catalog->closed)
		ret = -ESTALE;
	else if (id && (index < 0 || !catalog->entries[index].selectable))
		ret = -ESTALE;
	else if (catalog->info.suggested_id == id)
		ret = 0;
	else if (catalog->info.generation == U64_MAX)
		ret = -EOVERFLOW;
	else {
		catalog->info.suggested_id = id;
		catalog->info.generation++;
	}
	mutex_unlock(&catalog->lock);
	return ret;
}
EXPORT_SYMBOL_GPL(drm_constraints_catalog_suggest);

static int validate_entry(struct drm_constraints_catalog *catalog,
			  struct drm_constraints_entry *entry)
{
	int index;
	u64 id;

	lockdep_assert_held(&catalog->lock);
	if (catalog->closed)
		return -ESTALE;
	if (!entry)
		return -EINVAL;
	id = drm_constraints_entry_id(entry);
	index = find_entry(catalog, id);
	if (index < 0 || catalog->entries[index].entry != entry)
		return -ESTALE;
	if (!catalog->entries[index].selectable && catalog->info.selected_id != id)
		return -ESTALE;
	return 0;
}

struct drm_constraints_entry *
drm_constraints_catalog_lookup(struct drm_constraints_catalog *catalog, u64 id)
{
	struct drm_constraints_entry *entry;
	int index, ret;

	if (!id)
		return ERR_PTR(-EINVAL);
	mutex_lock(&catalog->lock);
	index = find_entry(catalog, id);
	if (index < 0) {
		entry = ERR_PTR(-ESTALE);
	} else {
		entry = catalog->entries[index].entry;
		ret = validate_entry(catalog, entry);
		entry = ret ? ERR_PTR(ret) : drm_constraints_entry_get(entry);
	}
	mutex_unlock(&catalog->lock);
	return entry;
}
EXPORT_SYMBOL_GPL(drm_constraints_catalog_lookup);

int drm_constraints_catalog_check(struct drm_constraints_catalog *catalog,
				  struct drm_constraints_entry *entry,
				  int (*check)(struct drm_constraints_entry *, void *), void *data)
{
	int ret;

	if (!check)
		return -EINVAL;
	mutex_lock(&catalog->lock);
	ret = validate_entry(catalog, entry);
	if (!ret)
		ret = check(entry, data);
	mutex_unlock(&catalog->lock);
	return ret > 0 ? -EINVAL : ret;
}
EXPORT_SYMBOL_GPL(drm_constraints_catalog_check);

int drm_constraints_catalog_accept(struct drm_constraints_catalog *catalog,
				   struct drm_constraints_entry *entry,
				   int (*install)(struct drm_constraints_entry *, void *), void *data)
{
	int ret;
	bool changed;

	if (!install)
		return -EINVAL;
	mutex_lock(&catalog->lock);
	ret = validate_entry(catalog, entry);
	if (ret)
		goto out;
	changed = catalog->info.selected_id != drm_constraints_entry_id(entry);
	if (changed && catalog->info.generation == U64_MAX) {
		ret = -EOVERFLOW;
		goto out;
	}
	ret = install(entry, data);
	if (!ret && changed) {
		catalog->info.selected_id = drm_constraints_entry_id(entry);
		catalog->info.generation++;
	}
out:
	mutex_unlock(&catalog->lock);
	return ret > 0 ? -EINVAL : ret;
}
EXPORT_SYMBOL_GPL(drm_constraints_catalog_accept);

struct drm_constraints_snapshot *
drm_constraints_catalog_snapshot(struct drm_constraints_catalog *catalog, u64 generation)
{
	struct drm_constraints_snapshot *snapshot;
	unsigned int i;

	snapshot = kzalloc(struct_size(snapshot, entries, catalog->limit), GFP_KERNEL);
	if (!snapshot)
		return ERR_PTR(-ENOMEM);
	mutex_lock(&catalog->lock);
	if (catalog->closed || (generation && generation != catalog->info.generation)) {
		mutex_unlock(&catalog->lock);
		kfree(snapshot);
		return ERR_PTR(-ESTALE);
	}
	snapshot->info = catalog->info;
	for (i = 0; i < snapshot->info.count; i++) {
		snapshot->entries[i] = catalog->entries[i];
		drm_constraints_entry_get(snapshot->entries[i].entry);
	}
	mutex_unlock(&catalog->lock);
	return snapshot;
}
EXPORT_SYMBOL_GPL(drm_constraints_catalog_snapshot);

void drm_constraints_snapshot_put(struct drm_constraints_snapshot *snapshot)
{
	unsigned int i;

	for (i = 0; i < snapshot->info.count; i++)
		drm_constraints_entry_put(snapshot->entries[i].entry);
	kfree(snapshot);
}
EXPORT_SYMBOL_GPL(drm_constraints_snapshot_put);

const struct drm_constraints_snapshot_info *
drm_constraints_snapshot_info(const struct drm_constraints_snapshot *snapshot)
{
	return &snapshot->info;
}
EXPORT_SYMBOL_GPL(drm_constraints_snapshot_info);

const struct drm_constraints_listing *
drm_constraints_snapshot_entries(const struct drm_constraints_snapshot *snapshot)
{
	return snapshot->entries;
}
EXPORT_SYMBOL_GPL(drm_constraints_snapshot_entries);
