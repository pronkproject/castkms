// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/err.h>
#include <linux/export.h>
#include <linux/kref.h>
#include <linux/mutex.h>
#include <linux/slab.h>
#include <drm/drm_atomic_prepare_owner.h>

#include "drm_atomic_prepare_owner_internal.h"

struct drm_prepare_owner {
	struct kref ref;
	struct mutex lock;
	struct list_head tickets;
	unsigned int count;
	unsigned int limit;
	bool revoked;
};

struct drm_prepare_owner *drm_prepare_owner_create(unsigned int max_tickets)
{
	struct drm_prepare_owner *owner;

	if (!max_tickets)
		return ERR_PTR(-EINVAL);
	owner = kzalloc_obj(*owner);
	if (!owner)
		return ERR_PTR(-ENOMEM);
	kref_init(&owner->ref);
	mutex_init(&owner->lock);
	INIT_LIST_HEAD(&owner->tickets);
	owner->limit = max_tickets;
	return owner;
}
EXPORT_SYMBOL_GPL(drm_prepare_owner_create);

struct drm_prepare_owner *drm_prepare_owner_get(struct drm_prepare_owner *owner)
{
	kref_get(&owner->ref);
	return owner;
}
EXPORT_SYMBOL_GPL(drm_prepare_owner_get);

static void owner_free(struct kref *ref)
{
	struct drm_prepare_owner *owner = container_of(ref, struct drm_prepare_owner, ref);

	WARN_ON(!list_empty(&owner->tickets));
	mutex_destroy(&owner->lock);
	kfree(owner);
}

void drm_prepare_owner_put(struct drm_prepare_owner *owner)
{
	kref_put(&owner->ref, owner_free);
}
EXPORT_SYMBOL_GPL(drm_prepare_owner_put);

void drm_prepare_owner_revoke(struct drm_prepare_owner *owner)
{
	struct drm_prepare_owner_registration *registration;

	mutex_lock(&owner->lock);
	if (!owner->revoked) {
		owner->revoked = true;
		list_for_each_entry(registration, &owner->tickets, link)
			registration->cancel(registration->data);
	}
	mutex_unlock(&owner->lock);
}
EXPORT_SYMBOL_GPL(drm_prepare_owner_revoke);

int drm_prepare_owner_register(struct drm_prepare_owner *owner,
			       struct drm_prepare_owner_registration *registration,
			       void (*cancel)(void *data), void *data)
{
	int ret = 0;

	mutex_lock(&owner->lock);
	if (owner->revoked) {
		ret = -ECANCELED;
	} else if (owner->count == owner->limit) {
		ret = -ENOSPC;
	} else {
		registration->cancel = cancel;
		registration->data = data;
		list_add_tail(&registration->link, &owner->tickets);
		owner->count++;
		drm_prepare_owner_get(owner);
	}
	mutex_unlock(&owner->lock);
	return ret;
}

void drm_prepare_owner_unregister(struct drm_prepare_owner *owner,
				  struct drm_prepare_owner_registration *registration)
{
	mutex_lock(&owner->lock);
	list_del(&registration->link);
	owner->count--;
	mutex_unlock(&owner->lock);
	drm_prepare_owner_put(owner);
}

int drm_prepare_owner_lock_live(struct drm_prepare_owner *owner)
{
	mutex_lock(&owner->lock);
	if (owner->revoked) {
		mutex_unlock(&owner->lock);
		return -ECANCELED;
	}
	return 0;
}

void drm_prepare_owner_unlock(struct drm_prepare_owner *owner)
{
	mutex_unlock(&owner->lock);
}
