// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/dma-fence.h>
#include <linux/completion.h>
#include <linux/err.h>
#include <linux/kthread.h>
#include <linux/module.h>
#include <drm/drm_atomic_prepare.h>
#include <kunit/test.h>

static const char *fence_name(struct dma_fence *fence)
{
	return "drm-prepare-test";
}

static const struct dma_fence_ops fence_ops = {
	.get_driver_name = fence_name,
	.get_timeline_name = fence_name,
};

static void put_fence(void *fence)
{
	dma_fence_put(fence);
}

static struct dma_fence *new_fence(struct kunit *test)
{
	struct dma_fence *fence = kzalloc_obj(*fence);

	KUNIT_ASSERT_NOT_NULL(test, fence);
	dma_fence_init(fence, &fence_ops, NULL, dma_fence_context_alloc(1), 1);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_fence, fence), 0);
	return fence;
}

static void put_source(void *source)
{
	drm_prepare_source_put(source);
}

static struct drm_prepare_source *new_source(struct kunit *test, unsigned int capacity)
{
	struct drm_prepare_source *source = drm_prepare_source_create(capacity);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, source);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_source, source), 0);
	return source;
}

static void abandon_read(void *read)
{
	drm_prepare_read_abandon(read);
}

static struct drm_prepare_read_claim *claim_read(struct kunit *test, struct drm_prepare_source *source)
{
	struct drm_prepare_read_claim *read = drm_prepare_source_claim(source);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, read);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, abandon_read, read), 0);
	return read;
}

static void release_read(struct kunit *test, struct drm_prepare_read_claim *read, struct dma_fence *fence)
{
	kunit_remove_action(test, abandon_read, read);
	drm_prepare_read_release(read, fence);
}

static void empty_source(struct kunit *test)
{
	struct drm_prepare_source *source = new_source(test, 1);
	struct dma_fence *fence = ERR_PTR(-EINVAL);

	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_source_create(0)), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_prepare_source_ready(source), -EAGAIN);
	KUNIT_EXPECT_EQ(test, drm_prepare_source_completion(source, &fence), -EAGAIN);
	KUNIT_EXPECT_PTR_EQ(test, fence, ERR_PTR(-EINVAL));
	drm_prepare_source_seal(source);
	drm_prepare_source_seal(source);
	KUNIT_EXPECT_EQ(test, drm_prepare_source_ready(source), 0);
	KUNIT_ASSERT_EQ(test, drm_prepare_source_completion(source, &fence), 0);
	KUNIT_EXPECT_PTR_EQ(test, fence, NULL);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_source_claim(source)), -EBUSY);
}

static void claimed_access_blocks_readiness(struct kunit *test)
{
	struct drm_prepare_source *source = new_source(test, 1);
	struct drm_prepare_read_claim *read = claim_read(test, source);

	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_source_claim(source)), -EAGAIN);
	drm_prepare_source_seal(source);
	KUNIT_EXPECT_EQ(test, drm_prepare_source_ready(source), -EAGAIN);
	release_read(test, read, NULL);
	KUNIT_EXPECT_EQ(test, drm_prepare_source_ready(source), 0);
}

static void ready_precedes_native_completion(struct kunit *test)
{
	struct drm_prepare_source *source = new_source(test, 2);
	struct drm_prepare_read_claim *first = claim_read(test, source);
	struct drm_prepare_read_claim *second = claim_read(test, source);
	struct dma_fence *a = new_fence(test), *b = new_fence(test), *completion;

	drm_prepare_source_seal(source);
	release_read(test, first, a);
	KUNIT_EXPECT_EQ(test, drm_prepare_source_ready(source), -EAGAIN);
	release_read(test, second, b);
	KUNIT_EXPECT_EQ(test, drm_prepare_source_ready(source), 0);
	KUNIT_ASSERT_EQ(test, drm_prepare_source_completion(source, &completion), 0);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_fence, completion), 0);
	KUNIT_EXPECT_FALSE(test, dma_fence_is_signaled(completion));
	dma_fence_signal(a);
	KUNIT_EXPECT_FALSE(test, dma_fence_is_signaled(completion));
	dma_fence_signal(b);
	KUNIT_EXPECT_TRUE(test, dma_fence_is_signaled(completion));
}

static void submitted_reads_keep_credit_until_completion(struct kunit *test)
{
	struct drm_prepare_source *source = new_source(test, 1);
	struct drm_prepare_read_claim *read = claim_read(test, source);
	struct dma_fence *fence = new_fence(test);

	release_read(test, read, fence);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_source_claim(source)), -EAGAIN);
	dma_fence_set_error(fence, -EIO);
	dma_fence_signal(fence);
	read = claim_read(test, source);
	release_read(test, read, NULL);
	drm_prepare_source_seal(source);
	KUNIT_EXPECT_EQ(test, drm_prepare_source_ready(source), 0);
}

static void abandoned_claim_is_terminal_failure(struct kunit *test)
{
	struct drm_prepare_source *source = new_source(test, 2);
	struct drm_prepare_read_claim *lost = claim_read(test, source);
	struct drm_prepare_read_claim *other = claim_read(test, source);
	struct dma_fence *fence = ERR_PTR(-EINVAL);

	drm_prepare_source_seal(source);
	kunit_release_action(test, abandon_read, lost);
	KUNIT_EXPECT_EQ(test, drm_prepare_source_ready(source), -EIO);
	release_read(test, other, NULL);
	KUNIT_EXPECT_EQ(test, drm_prepare_source_ready(source), -EIO);
	KUNIT_EXPECT_EQ(test, drm_prepare_source_completion(source, &fence), -EIO);
	KUNIT_EXPECT_PTR_EQ(test, fence, ERR_PTR(-EINVAL));
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_source_claim(source)), -EIO);
}

static void claim_retains_source_after_owner_put(struct kunit *test)
{
	struct drm_prepare_source *source = new_source(test, 1);
	struct drm_prepare_read_claim *read = claim_read(test, source);

	kunit_release_action(test, put_source, source);
	/* The claim independently retains the source until the consuming release. */
	drm_prepare_source_seal(source);
	KUNIT_EXPECT_EQ(test, drm_prepare_source_ready(source), -EAGAIN);
	release_read(test, read, NULL);
}

struct claim_race {
	struct drm_prepare_source *source;
	struct drm_prepare_read_claim *read;
	struct completion start;
	struct completion done;
};

static int race_claim(void *data)
{
	struct claim_race *race = data;

	wait_for_completion(&race->start);
	race->read = drm_prepare_source_claim(race->source);
	complete(&race->done);
	/* Keep the task alive until its owner joins it with kthread_stop(). */
	while (!kthread_should_stop())
		schedule_timeout_interruptible(1);
	return 0;
}

static void seal_serializes_with_admission(struct kunit *test)
{
	unsigned int iteration;

	for (iteration = 0; iteration < 32; iteration++) {
		struct claim_race race = { .source = new_source(test, 1) };
		struct task_struct *worker;
		unsigned long finished;

		init_completion(&race.start);
		init_completion(&race.done);
		worker = kthread_run(race_claim, &race, "prepare-claim");
		KUNIT_ASSERT_NOT_ERR_OR_NULL(test, worker);
		complete(&race.start);
		drm_prepare_source_seal(race.source);
		finished = wait_for_completion_timeout(&race.done, HZ);
		kthread_stop(worker);
		if (!finished) {
			if (!IS_ERR_OR_NULL(race.read))
				drm_prepare_read_abandon(race.read);
			KUNIT_FAIL(test, "claim worker did not finish");
			return;
		}
		if (IS_ERR(race.read)) {
			KUNIT_EXPECT_EQ(test, PTR_ERR(race.read), -EBUSY);
		} else {
			KUNIT_EXPECT_EQ(test, drm_prepare_source_ready(race.source), -EAGAIN);
			drm_prepare_read_release(race.read, NULL);
		}
		KUNIT_EXPECT_EQ(test, drm_prepare_source_ready(race.source), 0);
	}
}

static struct kunit_case cases[] = {
	KUNIT_CASE(empty_source),
	KUNIT_CASE(claimed_access_blocks_readiness),
	KUNIT_CASE(ready_precedes_native_completion),
	KUNIT_CASE(submitted_reads_keep_credit_until_completion),
	KUNIT_CASE(abandoned_claim_is_terminal_failure),
	KUNIT_CASE(claim_retains_source_after_owner_put),
	KUNIT_CASE(seal_serializes_with_admission),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_prepare",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_LICENSE("GPL");
