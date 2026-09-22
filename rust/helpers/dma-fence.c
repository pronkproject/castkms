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

__rust_helper ktime_t rust_helper_dma_fence_timestamp(struct dma_fence *fence)
{
	return dma_fence_timestamp(fence);
}

__rust_helper bool rust_helper_dma_fence_begin_signalling(void)
{
	return dma_fence_begin_signalling();
}

__rust_helper void rust_helper_dma_fence_end_signalling(bool cookie)
{
	dma_fence_end_signalling(cookie);
}

__rust_helper bool rust_helper_dma_fence_is_signaled(struct dma_fence *fence)
{
	return dma_fence_is_signaled(fence);
}

__rust_helper bool rust_helper_dma_fence_test_signaled_flag(struct dma_fence *fence)
{
	return dma_fence_test_signaled_flag(fence);
}

__rust_helper void rust_helper_dma_fence_lock_irqsave(struct dma_fence *fence,
							       unsigned long *flags)
{
	dma_fence_lock_irqsave(fence, *flags);
}

__rust_helper void rust_helper_dma_fence_unlock_irqrestore(struct dma_fence *fence,
								    unsigned long *flags)
{
	dma_fence_unlock_irqrestore(fence, *flags);
}

__rust_helper void rust_helper_dma_fence_set_error(struct dma_fence *fence, int error)
{
	dma_fence_set_error(fence, error);
}
