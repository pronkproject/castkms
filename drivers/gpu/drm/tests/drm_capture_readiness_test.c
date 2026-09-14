// SPDX-License-Identifier: GPL-2.0-only

#include <linux/module.h>
#include <linux/wait.h>
#include <drm/drm_capture_readiness.h>
#include <kunit/test.h>

static void readiness_put(void *data)
{
	drm_capture_readiness_put(data);
}

static struct drm_capture_readiness *readiness_create(struct kunit *test)
{
	struct drm_capture_readiness *readiness = drm_capture_readiness_create();

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, readiness);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, readiness_put, readiness), 0);
	return readiness;
}

static void readiness_outlives_an_independent_reference(struct kunit *test)
{
	struct drm_capture_readiness *readiness = readiness_create(test);
	struct drm_capture_readiness *retained = drm_capture_readiness_get(readiness);

	KUNIT_EXPECT_FALSE(test, drm_capture_readiness_has_results(readiness));
	drm_capture_readiness_update(readiness, true);
	drm_capture_readiness_put(retained);
	KUNIT_EXPECT_TRUE(test, drm_capture_readiness_has_results(readiness));
	drm_capture_readiness_update(readiness, false);
	KUNIT_EXPECT_FALSE(test, drm_capture_readiness_has_results(readiness));
}

struct readiness_waiter {
	wait_queue_entry_t wait;
	struct drm_capture_readiness *readiness;
	unsigned int wakeups;
	bool observed_not_ready;
};

static int readiness_wake(wait_queue_entry_t *entry, unsigned int mode, int flags, void *key)
{
	struct readiness_waiter *waiter = container_of(entry, struct readiness_waiter, wait);

	waiter->wakeups++;
	if (!drm_capture_readiness_has_results(waiter->readiness))
		waiter->observed_not_ready = true;
	return 0;
}

static void readiness_publishes_before_waking_registered_waiters(struct kunit *test)
{
	struct drm_capture_readiness *readiness = readiness_create(test);
	struct readiness_waiter waiter = { .readiness = readiness };
	wait_queue_head_t *queue = drm_capture_readiness_waitqueue(readiness);

	init_waitqueue_func_entry(&waiter.wait, readiness_wake);
	add_wait_queue(queue, &waiter.wait);
	KUNIT_EXPECT_FALSE(test, drm_capture_readiness_has_results(readiness));
	drm_capture_readiness_update(readiness, true);
	KUNIT_EXPECT_EQ(test, waiter.wakeups, 1);
	KUNIT_EXPECT_TRUE(test, drm_capture_readiness_has_results(readiness));
	drm_capture_readiness_update(readiness, false);
	KUNIT_EXPECT_FALSE(test, drm_capture_readiness_has_results(readiness));
	drm_capture_readiness_update(readiness, true);
	KUNIT_EXPECT_EQ(test, waiter.wakeups, 2);
	KUNIT_EXPECT_FALSE(test, waiter.observed_not_ready);
	remove_wait_queue(queue, &waiter.wait);
}

static struct kunit_case readiness_cases[] = {
	KUNIT_CASE(readiness_outlives_an_independent_reference),
	KUNIT_CASE(readiness_publishes_before_waking_registered_waiters),
	{}
};

static struct kunit_suite readiness_suite = {
	.name = "drm_capture_readiness",
	.test_cases = readiness_cases,
};
kunit_test_suite(readiness_suite);

MODULE_LICENSE("GPL");
