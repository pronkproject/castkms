// SPDX-License-Identifier: GPL-2.0-only
/* Result availability without provider callbacks or result ownership. */

#include <linux/atomic.h>
#include <linux/err.h>
#include <linux/export.h>
#include <linux/kref.h>
#include <linux/poll.h>
#include <linux/slab.h>

#include <drm/drm_capture_readiness.h>

struct drm_capture_readiness {
	struct kref ref;
	atomic_t ready;
	wait_queue_head_t wait;
};

static void readiness_release(struct kref *ref)
{
	struct drm_capture_readiness *readiness =
		container_of(ref, struct drm_capture_readiness, ref);

	kfree(readiness);
}

struct drm_capture_readiness *drm_capture_readiness_create(void)
{
	struct drm_capture_readiness *readiness = kzalloc_obj(*readiness);

	if (!readiness)
		return ERR_PTR(-ENOMEM);
	kref_init(&readiness->ref);
	atomic_set(&readiness->ready, 0);
	init_waitqueue_head(&readiness->wait);
	return readiness;
}
EXPORT_SYMBOL_GPL(drm_capture_readiness_create);

struct drm_capture_readiness *drm_capture_readiness_get(struct drm_capture_readiness *readiness)
{
	kref_get(&readiness->ref);
	return readiness;
}
EXPORT_SYMBOL_GPL(drm_capture_readiness_get);

void drm_capture_readiness_put(struct drm_capture_readiness *readiness)
{
	if (readiness)
		kref_put(&readiness->ref, readiness_release);
}
EXPORT_SYMBOL_GPL(drm_capture_readiness_put);

void drm_capture_readiness_update(struct drm_capture_readiness *readiness, bool ready)
{
	atomic_set_release(&readiness->ready, ready);
	if (ready)
		wake_up_interruptible_poll(&readiness->wait, EPOLLIN | EPOLLRDNORM);
}
EXPORT_SYMBOL_GPL(drm_capture_readiness_update);

bool drm_capture_readiness_has_results(struct drm_capture_readiness *readiness)
{
	return atomic_read_acquire(&readiness->ready);
}
EXPORT_SYMBOL_GPL(drm_capture_readiness_has_results);

struct wait_queue_head *drm_capture_readiness_waitqueue(struct drm_capture_readiness *readiness)
{
	return &readiness->wait;
}
EXPORT_SYMBOL_GPL(drm_capture_readiness_waitqueue);
