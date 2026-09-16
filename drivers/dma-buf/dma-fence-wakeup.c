// SPDX-License-Identifier: GPL-2.0 OR MIT
/* Cancellable waitqueue notification for concrete native completion. */

#include <linux/dma-fence.h>
#include <linux/dma-fence-wakeup.h>
#include <linux/err.h>
#include <linux/export.h>
#include <linux/slab.h>

struct dma_fence_wakeup {
	struct dma_fence_cb callback;
	struct dma_fence *fence;
	wait_queue_head_t *wait;
};

static void fence_wakeup(struct dma_fence *fence, struct dma_fence_cb *callback)
{
	struct dma_fence_wakeup *wakeup =
		container_of(callback, struct dma_fence_wakeup, callback);

	wake_up_all(wakeup->wait);
}

struct dma_fence_wakeup *dma_fence_wakeup_create(struct dma_fence *fence,
					       wait_queue_head_t *wait)
{
	struct dma_fence_wakeup *wakeup;

	if (!fence || !wait)
		return ERR_PTR(-EINVAL);
	wakeup = kzalloc_obj(*wakeup);
	if (!wakeup)
		return ERR_PTR(-ENOMEM);
	wakeup->fence = dma_fence_get(fence);
	wakeup->wait = wait;
	if (dma_fence_add_callback(fence, &wakeup->callback, fence_wakeup))
		wake_up_all(wait);
	return wakeup;
}
EXPORT_SYMBOL_GPL(dma_fence_wakeup_create);

void dma_fence_wakeup_destroy(struct dma_fence_wakeup *wakeup)
{
	/* Removal takes the signaling lock and waits out any running callback. */
	dma_fence_remove_callback(wakeup->fence, &wakeup->callback);
	dma_fence_put(wakeup->fence);
	kfree(wakeup);
}
EXPORT_SYMBOL_GPL(dma_fence_wakeup_destroy);
