/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __LINUX_DMA_FENCE_RETIRE_H
#define __LINUX_DMA_FENCE_RETIRE_H

struct dma_fence;
struct dma_fence_retirement;
struct module;

/**
 * struct dma_fence_retirement_ops - owned cleanup after native access
 * @owner: Module retaining the callback and all payload destruction code.
 * @release: Consume the payload exactly once, in sleepable context.
 *
 * The callback must not wait for unrelated future work. Completion errors do not
 * suppress cleanup: a signaled fence ends access, not necessarily valid pixels.
 */
struct dma_fence_retirement_ops {
	struct module *owner;
	void (*release)(void *data);
};

/**
 * dma_fence_retirement_create - preallocate an independent cleanup owner
 * @ops: Immutable callbacks, retained by their module until release returns.
 * @data: Payload transferred only on successful creation.
 *
 * Allocate before admitting native access. Callers bound outstanding records and
 * storage themselves. Returns an owned record or an error pointer.
 */
struct dma_fence_retirement *
dma_fence_retirement_create(const struct dma_fence_retirement_ops *ops, void *data);

/**
 * dma_fence_retirement_submit - consume a record using concrete completion
 * @retirement: Unique unsubmitted record, consumed by the call.
 * @fence: Submitted completion covering all access to the retained payload.
 *
 * Takes a fence reference and arranges release after signaling, including errors
 * and fences already signaled. Does not allocate or wait. The callback runs on
 * the system unbound workqueue, outside the fence signaling lock.
 */
void dma_fence_retirement_submit(struct dma_fence_retirement *retirement,
				 struct dma_fence *fence);

/**
 * dma_fence_retirement_destroy - release an unused record synchronously
 * @retirement: Unique unsubmitted record, consumed by the call.
 *
 * No native access may remain. The caller must permit payload destruction to
 * sleep. Submitted records belong to completion and must not be destroyed here.
 */
void dma_fence_retirement_destroy(struct dma_fence_retirement *retirement);

#endif
