// SPDX-License-Identifier: GPL-2.0 OR MIT
/* Destination callbacks detached from client queue and file teardown. */

#include <linux/atomic.h>
#include <linux/export.h>
#include <linux/module.h>
#include <linux/slab.h>
#include <linux/workqueue.h>

#include <drm/drm_capture_delivery.h>

#include "drm_internal.h"

#define DRM_CAPTURE_DELIVERY_LIMIT 64

static struct workqueue_struct *delivery_queue;
static atomic_t deliveries = ATOMIC_INIT(0);

struct capture_delivery {
	struct work_struct work;
	const struct drm_capture_delivery_ops *ops;
	void *data;
};

static void capture_delivery_run(struct work_struct *work)
{
	struct capture_delivery *delivery = container_of(work, struct capture_delivery, work);
	struct module *owner = delivery->ops->owner;

	delivery->ops->run(delivery->data);
	kfree(delivery);
	atomic_dec(&deliveries);
	/* No provider code or payload is accessed after releasing its module. */
	module_put(owner);
}

int drm_capture_delivery_submit(const struct drm_capture_delivery_ops *ops, void *data)
{
	struct capture_delivery *delivery;
	int ret;

	if (!ops || !ops->run)
		return -EINVAL;
	if (!delivery_queue)
		return -ENODEV;
	if (!atomic_add_unless(&deliveries, 1, DRM_CAPTURE_DELIVERY_LIMIT))
		return -EAGAIN;
	if (!try_module_get(ops->owner)) {
		ret = -ENODEV;
		goto release_credit;
	}
	delivery = kzalloc_obj(*delivery);
	if (!delivery) {
		ret = -ENOMEM;
		goto release_module;
	}
	INIT_WORK(&delivery->work, capture_delivery_run);
	delivery->ops = ops;
	delivery->data = data;
	/* Each work allocation is new and is submitted exactly once. */
	queue_work(delivery_queue, &delivery->work);
	return 0;

release_module:
	module_put(ops->owner);
release_credit:
	atomic_dec(&deliveries);
	return ret;
}
EXPORT_SYMBOL_GPL(drm_capture_delivery_submit);

int drm_capture_delivery_init(void)
{
	delivery_queue = alloc_workqueue("drm-capture-delivery", WQ_UNBOUND, 8);
	return delivery_queue ? 0 : -ENOMEM;
}

void drm_capture_delivery_exit(void)
{
	if (delivery_queue)
		destroy_workqueue(delivery_queue);
	delivery_queue = NULL;
}
