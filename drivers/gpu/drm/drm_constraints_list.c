// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/err.h>
#include <linux/kref.h>
#include <linux/module.h>
#include <linux/mutex.h>
#include <linux/slab.h>

#include <drm/drm_constraints_list.h>
#include <drm/drm_constraints_entry.h>

#include "drm_constraints_internal.h"

struct drm_constraints_list {
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

struct drm_constraints_list *
drm_constraints_list_create(struct drm_constraints_domain *domain,
			       struct drm_constraints_entry *initial, unsigned int limit)
{
	struct drm_constraints_list *list;

	if (!domain || !initial || !limit || limit > DRM_CONSTRAINTS_MAX_ENTRIES ||
	    !drm_constraints_entry_in_domain(initial, domain))
		return ERR_PTR(-EINVAL);
	list = kzalloc(struct_size(list, entries, limit), GFP_KERNEL);
	if (!list)
		return ERR_PTR(-ENOMEM);
	kref_init(&list->ref);
	mutex_init(&list->lock);
	list->domain = drm_constraints_domain_get(domain);
	list->crtc_id = drm_constraints_entry_crtc(initial);
	list->limit = limit;
	list->info.generation = 1;
	list->info.selected_id = drm_constraints_entry_id(initial);
	list->info.count = 1;
	list->entries[0].entry = drm_constraints_entry_get(initial);
	list->entries[0].selectable = true;
	return list;
}
EXPORT_SYMBOL_GPL(drm_constraints_list_create);

struct drm_constraints_list *
drm_constraints_list_get(struct drm_constraints_list *list)
{
	kref_get(&list->ref);
	return list;
}
EXPORT_SYMBOL_GPL(drm_constraints_list_get);

static void list_free(struct kref *ref)
{
	struct drm_constraints_list *list = container_of(ref, struct drm_constraints_list, ref);
	unsigned int i;

	for (i = 0; i < list->info.count; i++)
		drm_constraints_entry_put(list->entries[i].entry);
	drm_constraints_domain_put(list->domain);
	mutex_destroy(&list->lock);
	kfree(list);
}

void drm_constraints_list_put(struct drm_constraints_list *list)
{
	kref_put(&list->ref, list_free);
}
EXPORT_SYMBOL_GPL(drm_constraints_list_put);

void drm_constraints_list_close(struct drm_constraints_list *list)
{
	mutex_lock(&list->lock);
	list->closed = true;
	mutex_unlock(&list->lock);
}
EXPORT_SYMBOL_GPL(drm_constraints_list_close);

static int find_entry(struct drm_constraints_list *list, u64 id)
{
	unsigned int i;

	lockdep_assert_held(&list->lock);
	for (i = 0; i < list->info.count; i++)
		if (drm_constraints_entry_id(list->entries[i].entry) == id)
			return i;
	return -ENOENT;
}

struct drm_constraints_entry *
drm_constraints_list_selected(struct drm_constraints_list *list)
{
	struct drm_constraints_entry *entry;
	int index;

	mutex_lock(&list->lock);
	index = find_entry(list, list->info.selected_id);
	entry = drm_constraints_entry_get(list->entries[index].entry);
	mutex_unlock(&list->lock);
	return entry;
}
EXPORT_SYMBOL_GPL(drm_constraints_list_selected);

int drm_constraints_list_quiesce(struct drm_constraints_list *list,
				    struct drm_constraints_entry *entry,
				    int (*quiesce)(struct drm_constraints_entry *, void *),
				    void *data)
{
	int index, ret;

	mutex_lock(&list->lock);
	index = find_entry(list, list->info.selected_id);
	if (list->entries[index].entry != entry)
		ret = -ESTALE;
	else
		ret = quiesce(entry, data);
	mutex_unlock(&list->lock);
	return ret > 0 ? -EINVAL : ret;
}

int drm_constraints_list_add(struct drm_constraints_list *list,
				struct drm_constraints_entry *entry)
{
	int ret = 0;

	if (!entry || !drm_constraints_entry_in_domain(entry, list->domain) ||
	    drm_constraints_entry_crtc(entry) != list->crtc_id)
		return -EINVAL;
	mutex_lock(&list->lock);
	if (list->closed)
		ret = -ESTALE;
	else if (find_entry(list, drm_constraints_entry_id(entry)) >= 0)
		ret = -EEXIST;
	else if (list->info.count == list->limit)
		ret = -ENOSPC;
	else if (list->info.generation == U64_MAX)
		ret = -EOVERFLOW;
	else {
		list->entries[list->info.count++] = (struct drm_constraints_listing) {
			.entry = drm_constraints_entry_get(entry),
			.selectable = true,
		};
		list->info.generation++;
	}
	mutex_unlock(&list->lock);
	return ret;
}
EXPORT_SYMBOL_GPL(drm_constraints_list_add);

int drm_constraints_list_withdraw(struct drm_constraints_list *list, u64 id)
{
	int index, ret = 0;

	mutex_lock(&list->lock);
	index = find_entry(list, id);
	if (index < 0)
		ret = index;
	else if (!list->entries[index].selectable)
		ret = 0;
	else if (list->info.generation == U64_MAX)
		ret = -EOVERFLOW;
	else {
		list->entries[index].selectable = false;
		if (list->info.suggested_id == id)
			list->info.suggested_id = 0;
		list->info.generation++;
	}
	mutex_unlock(&list->lock);
	return ret;
}
EXPORT_SYMBOL_GPL(drm_constraints_list_withdraw);

int drm_constraints_list_forget(struct drm_constraints_list *list, u64 id)
{
	struct drm_constraints_entry *entry = NULL;
	int index, ret = 0;

	mutex_lock(&list->lock);
	index = find_entry(list, id);
	if (index < 0)
		ret = index;
	else if (list->entries[index].selectable || list->info.selected_id == id)
		ret = -EBUSY;
	else if (list->info.generation == U64_MAX)
		ret = -EOVERFLOW;
	else {
		entry = list->entries[index].entry;
		list->info.count--;
		memmove(&list->entries[index], &list->entries[index + 1],
			(list->info.count - index) * sizeof(*list->entries));
		memset(&list->entries[list->info.count], 0, sizeof(*list->entries));
		list->info.generation++;
	}
	mutex_unlock(&list->lock);
	if (entry)
		drm_constraints_entry_put(entry);
	return ret;
}
EXPORT_SYMBOL_GPL(drm_constraints_list_forget);

int drm_constraints_list_suggest(struct drm_constraints_list *list, u64 id)
{
	int index, ret = 0;

	mutex_lock(&list->lock);
	index = find_entry(list, id);
	if (list->closed)
		ret = -ESTALE;
	else if (id && (index < 0 || !list->entries[index].selectable))
		ret = -ESTALE;
	else if (list->info.suggested_id == id)
		ret = 0;
	else if (list->info.generation == U64_MAX)
		ret = -EOVERFLOW;
	else {
		list->info.suggested_id = id;
		list->info.generation++;
	}
	mutex_unlock(&list->lock);
	return ret;
}
EXPORT_SYMBOL_GPL(drm_constraints_list_suggest);

static int validate_entry(struct drm_constraints_list *list,
			  struct drm_constraints_entry *entry)
{
	int index;
	u64 id;

	lockdep_assert_held(&list->lock);
	if (list->closed)
		return -ESTALE;
	if (!entry)
		return -EINVAL;
	id = drm_constraints_entry_id(entry);
	index = find_entry(list, id);
	if (index < 0 || list->entries[index].entry != entry)
		return -ESTALE;
	if (!list->entries[index].selectable && list->info.selected_id != id)
		return -ESTALE;
	return 0;
}

struct drm_constraints_entry *
drm_constraints_list_lookup(struct drm_constraints_list *list, u64 id)
{
	struct drm_constraints_entry *entry;
	int index, ret;

	if (!id)
		return ERR_PTR(-EINVAL);
	mutex_lock(&list->lock);
	index = find_entry(list, id);
	if (index < 0) {
		entry = ERR_PTR(-ESTALE);
	} else {
		entry = list->entries[index].entry;
		ret = validate_entry(list, entry);
		entry = ret ? ERR_PTR(ret) : drm_constraints_entry_get(entry);
	}
	mutex_unlock(&list->lock);
	return entry;
}
EXPORT_SYMBOL_GPL(drm_constraints_list_lookup);

int drm_constraints_list_check(struct drm_constraints_list *list,
				  struct drm_constraints_entry *entry,
				  int (*check)(struct drm_constraints_entry *, void *), void *data)
{
	int ret;

	if (!check)
		return -EINVAL;
	mutex_lock(&list->lock);
	ret = validate_entry(list, entry);
	if (!ret)
		ret = check(entry, data);
	mutex_unlock(&list->lock);
	return ret > 0 ? -EINVAL : ret;
}
EXPORT_SYMBOL_GPL(drm_constraints_list_check);

int drm_constraints_list_accept(struct drm_constraints_list *list,
				   struct drm_constraints_entry *entry,
				   int (*install)(struct drm_constraints_entry *, void *), void *data)
{
	int ret;
	bool changed;

	if (!install)
		return -EINVAL;
	mutex_lock(&list->lock);
	ret = validate_entry(list, entry);
	if (ret)
		goto out;
	changed = list->info.selected_id != drm_constraints_entry_id(entry);
	if (changed && list->info.generation == U64_MAX) {
		ret = -EOVERFLOW;
		goto out;
	}
	ret = install(entry, data);
	if (!ret && changed) {
		list->info.selected_id = drm_constraints_entry_id(entry);
		list->info.generation++;
	}
out:
	mutex_unlock(&list->lock);
	return ret > 0 ? -EINVAL : ret;
}
EXPORT_SYMBOL_GPL(drm_constraints_list_accept);

struct drm_constraints_snapshot *
drm_constraints_list_snapshot(struct drm_constraints_list *list, u64 generation)
{
	struct drm_constraints_snapshot *snapshot;
	unsigned int i;

	snapshot = kzalloc(struct_size(snapshot, entries, list->limit), GFP_KERNEL);
	if (!snapshot)
		return ERR_PTR(-ENOMEM);
	mutex_lock(&list->lock);
	if (list->closed || (generation && generation != list->info.generation)) {
		mutex_unlock(&list->lock);
		kfree(snapshot);
		return ERR_PTR(-ESTALE);
	}
	snapshot->info = list->info;
	for (i = 0; i < snapshot->info.count; i++) {
		snapshot->entries[i] = list->entries[i];
		drm_constraints_entry_get(snapshot->entries[i].entry);
	}
	mutex_unlock(&list->lock);
	return snapshot;
}
EXPORT_SYMBOL_GPL(drm_constraints_list_snapshot);

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
