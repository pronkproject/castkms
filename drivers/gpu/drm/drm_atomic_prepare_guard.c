// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/dma-fence.h>
#include <linux/err.h>
#include <linux/slab.h>
#include <drm/drm_atomic_prepare.h>

struct drm_prepare_retirement_guard {
	struct drm_prepare_retirement_set *set;
	struct dma_fence *completion;
};

struct drm_prepare_retirement_guard *
drm_prepare_retirement_guard_create(struct drm_prepare_retirement_set *set)
{
	struct drm_prepare_retirement_guard *guard;
	int error;

	error = drm_prepare_retirement_set_ready(set);
	if (error)
		return ERR_PTR(error);
	guard = kmalloc_obj(*guard);
	if (!guard)
		return ERR_PTR(-ENOMEM);
	error = drm_prepare_retirement_set_completion(set, &guard->completion);
	if (error) {
		kfree(guard);
		return ERR_PTR(error);
	}
	guard->set = drm_prepare_retirement_set_get(set);
	return guard;
}
EXPORT_SYMBOL_GPL(drm_prepare_retirement_guard_create);

void drm_prepare_retirement_guard_destroy(struct drm_prepare_retirement_guard *guard)
{
	dma_fence_put(guard->completion);
	drm_prepare_retirement_set_put(guard->set);
	kfree(guard);
}
EXPORT_SYMBOL_GPL(drm_prepare_retirement_guard_destroy);

struct dma_fence *
drm_prepare_retirement_guard_completion(struct drm_prepare_retirement_guard *guard)
{
	return guard->completion;
}
EXPORT_SYMBOL_GPL(drm_prepare_retirement_guard_completion);
