// SPDX-License-Identifier: GPL-2.0-only
/* Durable capture lifetime, independent of mode-specific request streams. */

#include <linux/completion.h>
#include <linux/err.h>
#include <linux/kref.h>
#include <linux/module.h>
#include <linux/mutex.h>
#include <linux/slab.h>
#include <linux/wait.h>

#include <drm/drm_capture_authority.h>

struct drm_capture_authority {
	struct kref ref;
	/* Serializes terminal revocation with provider resource admission. */
	struct mutex lock;
	struct completion cleanup_done;
	wait_queue_head_t wait;
	const struct drm_capture_authority_ops *ops;
	void *data;
	bool revoked;
};

struct drm_capture_authority *
drm_capture_authority_create(const struct drm_capture_authority_ops *ops, void *data)
{
	struct drm_capture_authority *authority;

	if (!ops || !ops->revoke)
		return ERR_PTR(-EINVAL);
	if (!try_module_get(ops->owner))
		return ERR_PTR(-ENODEV);
	authority = kzalloc_obj(*authority);
	if (!authority) {
		module_put(ops->owner);
		return ERR_PTR(-ENOMEM);
	}
	kref_init(&authority->ref);
	mutex_init(&authority->lock);
	init_completion(&authority->cleanup_done);
	init_waitqueue_head(&authority->wait);
	authority->ops = ops;
	authority->data = data;
	return authority;
}
EXPORT_SYMBOL_GPL(drm_capture_authority_create);

struct drm_capture_authority *drm_capture_authority_get(struct drm_capture_authority *authority)
{
	kref_get(&authority->ref);
	return authority;
}
EXPORT_SYMBOL_GPL(drm_capture_authority_get);

void drm_capture_authority_revoke(struct drm_capture_authority *authority)
{
	mutex_lock(&authority->lock);
	if (authority->revoked) {
		mutex_unlock(&authority->lock);
		wait_for_completion(&authority->cleanup_done);
		return;
	}
	WRITE_ONCE(authority->revoked, true);
	mutex_unlock(&authority->lock);

	authority->ops->revoke(authority->data);
	complete_all(&authority->cleanup_done);
	wake_up_all(&authority->wait);
}
EXPORT_SYMBOL_GPL(drm_capture_authority_revoke);

static void drm_capture_authority_release(struct kref *ref)
{
	struct drm_capture_authority *authority =
		container_of(ref, struct drm_capture_authority, ref);
	struct module *owner = authority->ops->owner;

	drm_capture_authority_revoke(authority);
	if (authority->ops->release)
		authority->ops->release(authority->data);
	mutex_destroy(&authority->lock);
	kfree(authority);
	module_put(owner);
}

void drm_capture_authority_put(struct drm_capture_authority *authority)
{
	kref_put(&authority->ref, drm_capture_authority_release);
}
EXPORT_SYMBOL_GPL(drm_capture_authority_put);

int drm_capture_authority_begin(struct drm_capture_authority *authority)
{
	mutex_lock(&authority->lock);
	if (authority->revoked) {
		mutex_unlock(&authority->lock);
		return -EKEYREVOKED;
	}
	return 0;
}
EXPORT_SYMBOL_GPL(drm_capture_authority_begin);

void drm_capture_authority_end(struct drm_capture_authority *authority)
{
	mutex_unlock(&authority->lock);
}
EXPORT_SYMBOL_GPL(drm_capture_authority_end);

bool drm_capture_authority_revoked(struct drm_capture_authority *authority)
{
	return READ_ONCE(authority->revoked);
}
EXPORT_SYMBOL_GPL(drm_capture_authority_revoked);

bool drm_capture_authority_cleanup_done(struct drm_capture_authority *authority)
{
	return completion_done(&authority->cleanup_done);
}
EXPORT_SYMBOL_GPL(drm_capture_authority_cleanup_done);

struct wait_queue_head *drm_capture_authority_waitqueue(struct drm_capture_authority *authority)
{
	return &authority->wait;
}
EXPORT_SYMBOL_GPL(drm_capture_authority_waitqueue);
