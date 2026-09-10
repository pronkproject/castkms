// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/dma-fence.h>
#include <linux/err.h>
#include <linux/export.h>
#include <linux/slab.h>
#include <drm/drm_atomic.h>
#include <drm/drm_atomic_prepare.h>
#include <drm/drm_atomic_prepare_commit.h>
#include <drm/drm_atomic_prepare_scope.h>
#include <drm/drm_atomic_prepare_ticket.h>

struct drm_atomic_preparation {
	struct drm_prepare_attempt *attempt;
	struct drm_prepare_retirement_guard *guard;
	drm_atomic_prepare_observe_fn observe;
};

static int commit_prepare(struct drm_atomic_commit *state,
			  struct drm_prepare_ticket *ticket,
			  drm_atomic_prepare_observe_fn observe)
{
	struct drm_atomic_preparation *preparation;
	struct drm_prepare_attempt *attempt;

	if (state->preparation)
		return -EBUSY;
	if (state->async_update)
		return -EOPNOTSUPP;

	preparation = kzalloc(sizeof(*preparation), GFP_KERNEL);
	if (!preparation)
		return -ENOMEM;
	attempt = drm_prepare_ticket_reserve(ticket);
	if (IS_ERR(attempt)) {
		kfree(preparation);
		return PTR_ERR(attempt);
	}
	preparation->attempt = attempt;
	preparation->observe = observe;
	state->preparation = preparation;
	return 0;
}

int drm_atomic_commit_prepare(struct drm_atomic_commit *state,
			     struct drm_prepare_ticket *ticket)
{
	return commit_prepare(state, ticket, NULL);
}
EXPORT_SYMBOL_GPL(drm_atomic_commit_prepare);

int drm_atomic_commit_prepare_scoped(struct drm_atomic_commit *state,
				    struct drm_prepare_ticket *ticket,
				    drm_atomic_prepare_observe_fn observe)
{
	if (!observe)
		return -EINVAL;
	return commit_prepare(state, ticket, observe);
}
EXPORT_SYMBOL_GPL(drm_atomic_commit_prepare_scoped);

static int install_scoped(struct drm_atomic_commit *state,
			  int (*install)(void *data))
{
	struct drm_atomic_preparation *preparation = state->preparation;
	struct drm_prepare_scope_entry entries[DRM_PREPARE_SCOPE_MAX_OUTPUTS];
	int count;

	count = preparation->observe(state, entries, ARRAY_SIZE(entries));
	if (count < 0)
		return count;
	if (count > ARRAY_SIZE(entries))
		return -E2BIG;
	return drm_prepare_attempt_commit_scoped(preparation->attempt, entries, count,
						install, state, &preparation->guard);
}

int drm_atomic_commit_preparation_install(struct drm_atomic_commit *state,
					 int (*install)(void *data))
{
	struct drm_atomic_preparation *preparation = state->preparation;
	int ret;

	if (!preparation)
		return install(state);
	if (!preparation->attempt)
		return -EALREADY;
	if (state->async_update)
		return -EOPNOTSUPP;

	if (preparation->observe)
		ret = install_scoped(state, install);
	else
		ret = drm_prepare_attempt_commit(preparation->attempt, install, state,
						 &preparation->guard);
	if (ret)
		return ret;
	drm_prepare_attempt_destroy(preparation->attempt);
	preparation->attempt = NULL;
	return 0;
}
EXPORT_SYMBOL_GPL(drm_atomic_commit_preparation_install);

void drm_atomic_commit_wait_for_readers(struct drm_atomic_commit *state)
{
	struct drm_atomic_preparation *preparation = state->preparation;
	struct dma_fence *fence;

	if (!preparation || !preparation->guard)
		return;
	fence = drm_prepare_retirement_guard_completion(preparation->guard);
	if (fence)
		dma_fence_wait(fence, false);
}
EXPORT_SYMBOL_GPL(drm_atomic_commit_wait_for_readers);

void drm_atomic_commit_preparation_clear(struct drm_atomic_commit *state)
{
	struct drm_atomic_preparation *preparation = state->preparation;

	if (!preparation)
		return;
	state->preparation = NULL;
	if (preparation->attempt)
		drm_prepare_attempt_destroy(preparation->attempt);
	if (preparation->guard)
		drm_prepare_retirement_guard_destroy(preparation->guard);
	kfree(preparation);
}
EXPORT_SYMBOL_GPL(drm_atomic_commit_preparation_clear);
