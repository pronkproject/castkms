// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <drm/drm_atomic_state_helper.h>
#include <drm/drm_atomic_uapi.h>
#include <drm/drm_plane.h>
#include <kunit/test.h>
#include <linux/dma-fence.h>

static const char *fence_name(struct dma_fence *fence)
{
	return "drm-atomic-input-test";
}

static const struct dma_fence_ops fence_ops = {
	.get_driver_name = fence_name,
	.get_timeline_name = fence_name,
};

static void put_fence(void *data)
{
	dma_fence_put(data);
}

static struct dma_fence *new_fence(struct kunit *test)
{
	struct dma_fence *fence = kzalloc_obj(*fence);

	KUNIT_ASSERT_NOT_NULL(test, fence);
	dma_fence_init(fence, &fence_ops, NULL, dma_fence_context_alloc(1), 1);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_fence, fence), 0);
	return fence;
}

static void destroy_state(void *data)
{
	__drm_atomic_helper_plane_destroy_state(data);
}

static struct drm_plane_state *new_state(struct kunit *test)
{
	struct drm_plane_state *state = kunit_kzalloc(test, sizeof(*state), GFP_KERNEL);

	KUNIT_ASSERT_NOT_NULL(test, state);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, destroy_state, state), 0);
	return state;
}

static void resolved_fence_survives_discarded_attempt(struct kunit *test)
{
	struct dma_fence *fence = new_fence(test);
	struct drm_plane_state *first = new_state(test);
	struct drm_plane_state *second;

	KUNIT_ASSERT_EQ(test, drm_atomic_set_fence_for_plane(first, fence), 0);
	KUNIT_EXPECT_PTR_EQ(test, first->fence, fence);
	KUNIT_EXPECT_EQ(test, kref_read(&fence->refcount), 2);
	kunit_release_action(test, destroy_state, first);
	KUNIT_EXPECT_EQ(test, kref_read(&fence->refcount), 1);
	second = new_state(test);
	KUNIT_ASSERT_EQ(test, drm_atomic_set_fence_for_plane(second, fence), 0);
	KUNIT_EXPECT_PTR_EQ(test, second->fence, fence);
	KUNIT_EXPECT_EQ(test, kref_read(&fence->refcount), 2);
}

static void existing_fence_cannot_be_replaced(struct kunit *test)
{
	struct dma_fence *first = new_fence(test);
	struct dma_fence *second = new_fence(test);
	struct drm_plane_state *state = new_state(test);

	KUNIT_ASSERT_EQ(test, drm_atomic_set_fence_for_plane(state, first), 0);
	KUNIT_EXPECT_EQ(test, drm_atomic_set_fence_for_plane(state, second), -EINVAL);
	KUNIT_EXPECT_PTR_EQ(test, state->fence, first);
	KUNIT_EXPECT_EQ(test, kref_read(&first->refcount), 2);
	KUNIT_EXPECT_EQ(test, kref_read(&second->refcount), 1);
}

static struct kunit_case cases[] = {
	KUNIT_CASE(resolved_fence_survives_discarded_attempt),
	KUNIT_CASE(existing_fence_cannot_be_replaced),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_fence",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_LICENSE("GPL");
