// SPDX-License-Identifier: GPL-2.0

#include <linux/dma-fence.h>

__rust_helper struct dma_fence *rust_helper_dma_fence_get(struct dma_fence *fence)
{
	return dma_fence_get(fence);
}

__rust_helper void rust_helper_dma_fence_put(struct dma_fence *fence)
{
	dma_fence_put(fence);
}
