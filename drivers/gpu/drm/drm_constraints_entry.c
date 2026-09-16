// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/err.h>
#include <linux/kref.h>
#include <linux/module.h>
#include <linux/mutex.h>
#include <linux/slab.h>

#include <drm/drm_constraints.h>
#include <drm/drm_constraints_entry.h>

struct drm_constraints_domain {
	struct kref ref;
	struct mutex lock;
	u64 last_id;
	unsigned int retained;
	unsigned int limit;
};

struct drm_constraints_entry {
	struct kref ref;
	struct drm_constraints_domain *domain;
	struct drm_constraints_description *description;
	const struct drm_constraints_entry_ops *ops;
	void *data;
	u64 id;
	u32 crtc_id;
};

struct drm_constraints_domain *drm_constraints_domain_create(unsigned int limit)
{
	struct drm_constraints_domain *domain;

	if (!limit)
		return ERR_PTR(-EINVAL);
	domain = kzalloc_obj(*domain);
	if (!domain)
		return ERR_PTR(-ENOMEM);
	kref_init(&domain->ref);
	mutex_init(&domain->lock);
	domain->limit = limit;
	return domain;
}
EXPORT_SYMBOL_GPL(drm_constraints_domain_create);

struct drm_constraints_domain *drm_constraints_domain_get(struct drm_constraints_domain *domain)
{
	kref_get(&domain->ref);
	return domain;
}
EXPORT_SYMBOL_GPL(drm_constraints_domain_get);

static void domain_free(struct kref *ref)
{
	struct drm_constraints_domain *domain = container_of(ref, struct drm_constraints_domain, ref);

	mutex_destroy(&domain->lock);
	kfree(domain);
}

void drm_constraints_domain_put(struct drm_constraints_domain *domain)
{
	kref_put(&domain->ref, domain_free);
}
EXPORT_SYMBOL_GPL(drm_constraints_domain_put);

struct drm_constraints_entry *
drm_constraints_entry_create(struct drm_constraints_domain *domain, u32 crtc_id,
			     struct drm_constraints_description *description,
			     const struct drm_constraints_entry_ops *ops, void *data)
{
	struct drm_constraints_entry *entry;
	int ret = 0;

	if (!domain || !crtc_id || !description || !ops || !ops->release)
		return ERR_PTR(-EINVAL);
	if (!try_module_get(ops->owner))
		return ERR_PTR(-ENODEV);
	entry = kzalloc_obj(*entry);
	if (!entry) {
		ret = -ENOMEM;
		goto put_module;
	}
	mutex_lock(&domain->lock);
	if (domain->last_id == U64_MAX)
		ret = -EOVERFLOW;
	else if (domain->retained == domain->limit)
		ret = -ENOSPC;
	else {
		entry->id = ++domain->last_id;
		domain->retained++;
	}
	mutex_unlock(&domain->lock);
	if (ret) {
		kfree(entry);
		goto put_module;
	}
	kref_init(&entry->ref);
	entry->domain = drm_constraints_domain_get(domain);
	entry->description = drm_constraints_description_get(description);
	entry->ops = ops;
	entry->data = data;
	entry->crtc_id = crtc_id;
	return entry;

put_module:
	module_put(ops->owner);
	return ERR_PTR(ret);
}
EXPORT_SYMBOL_GPL(drm_constraints_entry_create);

static void stateless_release(void *data)
{
}

static const struct drm_constraints_entry_ops stateless_ops = {
	.owner = THIS_MODULE,
	.release = stateless_release,
};

struct drm_constraints_entry *
drm_constraints_entry_create_stateless(struct drm_constraints_domain *domain, u32 crtc_id,
				      struct drm_constraints_description *description)
{
	return drm_constraints_entry_create(domain, crtc_id, description, &stateless_ops, NULL);
}
EXPORT_SYMBOL_GPL(drm_constraints_entry_create_stateless);

struct drm_constraints_entry *drm_constraints_entry_get(struct drm_constraints_entry *entry)
{
	kref_get(&entry->ref);
	return entry;
}
EXPORT_SYMBOL_GPL(drm_constraints_entry_get);

static void entry_free(struct kref *ref)
{
	struct drm_constraints_entry *entry = container_of(ref, struct drm_constraints_entry, ref);
	struct drm_constraints_domain *domain = entry->domain;
	struct module *owner = entry->ops->owner;

	entry->ops->release(entry->data);
	drm_constraints_description_put(entry->description);
	mutex_lock(&domain->lock);
	domain->retained--;
	mutex_unlock(&domain->lock);
	drm_constraints_domain_put(domain);
	kfree(entry);
	module_put(owner);
}

void drm_constraints_entry_put(struct drm_constraints_entry *entry)
{
	kref_put(&entry->ref, entry_free);
}
EXPORT_SYMBOL_GPL(drm_constraints_entry_put);

u64 drm_constraints_entry_id(const struct drm_constraints_entry *entry)
{
	return entry->id;
}
EXPORT_SYMBOL_GPL(drm_constraints_entry_id);

u32 drm_constraints_entry_crtc(const struct drm_constraints_entry *entry)
{
	return entry->crtc_id;
}
EXPORT_SYMBOL_GPL(drm_constraints_entry_crtc);

bool drm_constraints_entry_in_domain(const struct drm_constraints_entry *entry,
				     const struct drm_constraints_domain *domain)
{
	return entry->domain == domain;
}
EXPORT_SYMBOL_GPL(drm_constraints_entry_in_domain);

struct drm_constraints_description *
drm_constraints_entry_description(const struct drm_constraints_entry *entry)
{
	return entry->description;
}
EXPORT_SYMBOL_GPL(drm_constraints_entry_description);

void *drm_constraints_entry_data(const struct drm_constraints_entry *entry)
{
	return entry->data;
}
EXPORT_SYMBOL_GPL(drm_constraints_entry_data);
