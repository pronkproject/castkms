// SPDX-License-Identifier: GPL-2.0 OR MIT
/* Owned cleanup detached from native fence signaling context. */

#include <linux/dma-fence.h>
#include <linux/dma-fence-retire.h>
#include <linux/err.h>
#include <linux/export.h>
#include <linux/module.h>
#include <linux/slab.h>
#include <linux/workqueue.h>

struct dma_fence_retirement {
	struct dma_fence_cb callback;
	struct work_struct work;
	struct dma_fence *fence;
	const struct dma_fence_retirement_ops *ops;
	void *data;
};

void dma_fence_retirement_destroy(struct dma_fence_retirement *retirement)
{
	struct module *owner = retirement->ops->owner;

	retirement->ops->release(retirement->data);
	dma_fence_put(retirement->fence);
	kfree(retirement);
	/* Payload code has returned before its module becomes unloadable. */
	module_put(owner);
}
EXPORT_SYMBOL_GPL(dma_fence_retirement_destroy);

static void retirement_work(struct work_struct *work)
{
	struct dma_fence_retirement *retirement =
		container_of(work, struct dma_fence_retirement, work);

	dma_fence_retirement_destroy(retirement);
}

static void retirement_signaled(struct dma_fence *fence, struct dma_fence_cb *callback)
{
	struct dma_fence_retirement *retirement =
		container_of(callback, struct dma_fence_retirement, callback);

	queue_work(system_dfl_wq, &retirement->work);
}

struct dma_fence_retirement *
dma_fence_retirement_create(const struct dma_fence_retirement_ops *ops, void *data)
{
	struct dma_fence_retirement *retirement;

	if (!ops || !ops->release)
		return ERR_PTR(-EINVAL);
	if (!try_module_get(ops->owner))
		return ERR_PTR(-ENODEV);
	retirement = kzalloc_obj(*retirement);
	if (!retirement) {
		module_put(ops->owner);
		return ERR_PTR(-ENOMEM);
	}
	INIT_WORK(&retirement->work, retirement_work);
	retirement->ops = ops;
	retirement->data = data;
	return retirement;
}
EXPORT_SYMBOL_GPL(dma_fence_retirement_create);

void dma_fence_retirement_submit(struct dma_fence_retirement *retirement,
				 struct dma_fence *fence)
{
	retirement->fence = dma_fence_get(fence);
	/* On success the callback may consume the record before this call returns. */
	if (dma_fence_add_callback(fence, &retirement->callback, retirement_signaled))
		queue_work(system_dfl_wq, &retirement->work);
}
EXPORT_SYMBOL_GPL(dma_fence_retirement_submit);
