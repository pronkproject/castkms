// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/err.h>
#include <linux/export.h>
#include <linux/kref.h>
#include <linux/list.h>
#include <linux/mutex.h>
#include <linux/slab.h>

#include <drm/drm_capture_authority.h>
#include <drm/drm_capture_creator.h>

struct drm_capture_creator {
	struct kref ref;
	struct mutex lock;
	struct list_head grants;
	u32 count;
	u32 limit;
};

struct drm_capture_registration {
	struct kref ref;
	struct list_head link;
	struct drm_capture_creator *creator;
	struct drm_capture_authority *authority;
	bool tracked;
};

static void creator_release(struct kref *ref)
{
	struct drm_capture_creator *creator =
		container_of(ref, struct drm_capture_creator, ref);

	mutex_destroy(&creator->lock);
	kfree(creator);
}

static void registration_release(struct kref *ref)
{
	struct drm_capture_registration *registration =
		container_of(ref, struct drm_capture_registration, ref);

	drm_capture_authority_put(registration->authority);
	kfree(registration);
}

struct drm_capture_creator *drm_capture_creator_create(u32 limit)
{
	struct drm_capture_creator *creator;

	if (!limit)
		return ERR_PTR(-EINVAL);
	creator = kzalloc(sizeof(*creator), GFP_KERNEL);
	if (!creator)
		return ERR_PTR(-ENOMEM);
	kref_init(&creator->ref);
	mutex_init(&creator->lock);
	INIT_LIST_HEAD(&creator->grants);
	creator->limit = limit;
	return creator;
}
EXPORT_SYMBOL_GPL(drm_capture_creator_create);

struct drm_capture_registration *
drm_capture_creator_register(struct drm_capture_creator *creator,
			     struct drm_capture_authority *authority)
{
	struct drm_capture_registration *registration;

	registration = kzalloc(sizeof(*registration), GFP_KERNEL);
	if (!registration)
		return ERR_PTR(-ENOMEM);
	mutex_lock(&creator->lock);
	if (creator->count == creator->limit) {
		mutex_unlock(&creator->lock);
		kfree(registration);
		return ERR_PTR(-EBUSY);
	}
	kref_init(&registration->ref);
	/* The list owns a reference independently of the returned registration. */
	kref_get(&registration->ref);
	registration->authority = drm_capture_authority_get(authority);
	registration->creator = creator;
	registration->tracked = true;
	kref_get(&creator->ref);
	list_add_tail(&registration->link, &creator->grants);
	creator->count++;
	mutex_unlock(&creator->lock);
	return registration;
}
EXPORT_SYMBOL_GPL(drm_capture_creator_register);

void drm_capture_registration_remove(struct drm_capture_registration *registration)
{
	struct drm_capture_creator *creator = registration->creator;
	bool tracked;

	mutex_lock(&creator->lock);
	tracked = registration->tracked;
	if (tracked) {
		list_del(&registration->link);
		registration->tracked = false;
		creator->count--;
	}
	mutex_unlock(&creator->lock);
	if (tracked)
		kref_put(&registration->ref, registration_release);
	kref_put(&registration->ref, registration_release);
	kref_put(&creator->ref, creator_release);
}
EXPORT_SYMBOL_GPL(drm_capture_registration_remove);

void drm_capture_creator_close(struct drm_capture_creator *creator)
{
	struct drm_capture_registration *registration, *next;
	LIST_HEAD(retired);

	mutex_lock(&creator->lock);
	list_for_each_entry(registration, &creator->grants, link)
		registration->tracked = false;
	list_splice_init(&creator->grants, &retired);
	creator->count = 0;
	mutex_unlock(&creator->lock);
	list_for_each_entry_safe(registration, next, &retired, link) {
		list_del(&registration->link);
		drm_capture_authority_revoke(registration->authority);
		kref_put(&registration->ref, registration_release);
	}
	kref_put(&creator->ref, creator_release);
}
EXPORT_SYMBOL_GPL(drm_capture_creator_close);
