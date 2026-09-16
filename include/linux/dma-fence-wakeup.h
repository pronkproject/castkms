/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __LINUX_DMA_FENCE_WAKEUP_H
#define __LINUX_DMA_FENCE_WAKEUP_H

#include <linux/wait.h>

struct dma_fence;
struct dma_fence_wakeup;

/**
 * dma_fence_wakeup_create - observe native completion without retaining work
 * @fence: Concrete native completion to observe.
 * @wait: Pinned waitqueue which must outlive the returned observation.
 *
 * Allocate an observation and retain @fence. Signaling wakes all waiters,
 * including when @fence has already signaled. The caller must register waiters
 * before checking authoritative completion state to avoid missed notifications.
 * Returns an owned observation or an error pointer. May sleep.
 */
struct dma_fence_wakeup *dma_fence_wakeup_create(struct dma_fence *fence,
					       wait_queue_head_t *wait);

/**
 * dma_fence_wakeup_destroy - detach an observation without waiting for completion
 * @wakeup: Unique observation, consumed by the call.
 *
 * Serialize with an executing callback, then release the retained fence. No
 * callback can access the supplied waitqueue after return. This operation does
 * not signal the fence or prove that any native access ended. The caller must allow
 * fence destruction to sleep and must not hold the fence's signaling lock.
 */
void dma_fence_wakeup_destroy(struct dma_fence_wakeup *wakeup);

#endif
