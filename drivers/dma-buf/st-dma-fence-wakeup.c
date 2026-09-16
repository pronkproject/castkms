// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <kunit/test.h>
#include <linux/dma-fence.h>
#include <linux/dma-fence-wakeup.h>
#include <linux/slab.h>
#include <linux/workqueue.h>

static const char *wakeup_fence_name(struct dma_fence *fence)
{
	return "wakeup-test";
}

static const struct dma_fence_ops wakeup_fence_ops = {
	.get_driver_name = wakeup_fence_name,
	.get_timeline_name = wakeup_fence_name,
};

static struct dma_fence *wakeup_fence_create(void)
{
	struct dma_fence *fence = kmalloc_obj(*fence);

	if (fence)
		dma_fence_init(fence, &wakeup_fence_ops, NULL,
			       dma_fence_context_alloc(1), 1);
	return fence;
}

static int count_wakeup(struct wait_queue_entry *entry, unsigned int mode,
			int flags, void *key)
{
	atomic_inc(entry->private);
	return 0;
}

static void wakeup_completion(struct kunit *test)
{
	DECLARE_WAIT_QUEUE_HEAD_ONSTACK(wait);
	wait_queue_entry_t entry;
	atomic_t count = ATOMIC_INIT(0);
	struct dma_fence_wakeup *wakeup;
	struct dma_fence *fence;
	int early, error;

	init_waitqueue_func_entry(&entry, count_wakeup);
	entry.private = &count;
	add_wait_queue(&wait, &entry);
	for (early = 0; early < 2; early++) {
		for (error = 0; error < 2; error++) {
			fence = wakeup_fence_create();
			KUNIT_ASSERT_NOT_NULL(test, fence);
			atomic_set(&count, 0);
			if (error)
				dma_fence_set_error(fence, -EIO);
			if (early)
				dma_fence_signal(fence);
			wakeup = dma_fence_wakeup_create(fence, &wait);
			KUNIT_ASSERT_NOT_ERR_OR_NULL(test, wakeup);
			if (!early) {
				KUNIT_EXPECT_EQ(test, atomic_read(&count), 0);
				dma_fence_signal(fence);
			}
			KUNIT_EXPECT_EQ(test, atomic_read(&count), 1);
			dma_fence_wakeup_destroy(wakeup);
			KUNIT_EXPECT_EQ(test, dma_fence_get_status(fence), error ? -EIO : 1);
			dma_fence_put(fence);
		}
	}
	remove_wait_queue(&wait, &entry);
}

static void wakeup_detach(struct kunit *test)
{
	DECLARE_WAIT_QUEUE_HEAD_ONSTACK(wait);
	wait_queue_entry_t entry;
	atomic_t count = ATOMIC_INIT(0);
	struct dma_fence_wakeup *wakeup;
	struct dma_fence *fence = wakeup_fence_create();

	KUNIT_ASSERT_NOT_NULL(test, fence);
	init_waitqueue_func_entry(&entry, count_wakeup);
	entry.private = &count;
	add_wait_queue(&wait, &entry);
	wakeup = dma_fence_wakeup_create(fence, &wait);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, wakeup);
	dma_fence_wakeup_destroy(wakeup);
	KUNIT_EXPECT_EQ(test, dma_fence_get_status(fence), 0);
	dma_fence_signal(fence);
	KUNIT_EXPECT_EQ(test, atomic_read(&count), 0);
	remove_wait_queue(&wait, &entry);
	dma_fence_put(fence);
}

struct signaling_work {
	struct work_struct work;
	struct dma_fence *fence;
};

static void signal_work(struct work_struct *work)
{
	struct signaling_work *signal = container_of(work, struct signaling_work, work);

	dma_fence_signal(signal->fence);
}

static void wakeup_racing_detach(struct kunit *test)
{
	DECLARE_WAIT_QUEUE_HEAD_ONSTACK(wait);
	struct dma_fence_wakeup *wakeup;
	struct signaling_work signal;
	wait_queue_entry_t entry;
	atomic_t count = ATOMIC_INIT(0);
	int i, observed;

	init_waitqueue_func_entry(&entry, count_wakeup);
	entry.private = &count;
	add_wait_queue(&wait, &entry);
	for (i = 0; i < 256; i++) {
		signal.fence = wakeup_fence_create();
		KUNIT_ASSERT_NOT_NULL(test, signal.fence);
		INIT_WORK_ONSTACK(&signal.work, signal_work);
		atomic_set(&count, 0);
		queue_work(system_dfl_wq, &signal.work);
		wakeup = dma_fence_wakeup_create(signal.fence, &wait);
		KUNIT_EXPECT_NOT_ERR_OR_NULL(test, wakeup);
		if (IS_ERR_OR_NULL(wakeup)) {
			flush_work(&signal.work);
			destroy_work_on_stack(&signal.work);
			dma_fence_put(signal.fence);
			break;
		}
		dma_fence_wakeup_destroy(wakeup);
		observed = atomic_read(&count);
		flush_work(&signal.work);
		KUNIT_EXPECT_EQ(test, atomic_read(&count), observed);
		KUNIT_EXPECT_LE(test, observed, 1);
		destroy_work_on_stack(&signal.work);
		dma_fence_put(signal.fence);
	}
	remove_wait_queue(&wait, &entry);
}

static struct kunit_case dma_fence_wakeup_cases[] = {
	KUNIT_CASE(wakeup_completion),
	KUNIT_CASE(wakeup_detach),
	KUNIT_CASE(wakeup_racing_detach),
	{}
};

static struct kunit_suite dma_fence_wakeup_suite = {
	.name = "dma-fence-wakeup",
	.test_cases = dma_fence_wakeup_cases,
};

kunit_test_suite(dma_fence_wakeup_suite);

MODULE_LICENSE("GPL");
