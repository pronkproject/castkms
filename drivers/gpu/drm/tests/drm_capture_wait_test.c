// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/completion.h>
#include <linux/err.h>
#include <linux/kthread.h>
#include <linux/module.h>
#include <linux/sched/signal.h>
#include <drm/drm_capture.h>
#include <kunit/test.h>

struct capture_fixture {
	struct drm_capture *capture;
	struct drm_capture_job *job;
	u64 id;
};

static void free_fixture(void *data)
{
	struct capture_fixture *f = data;

	if (f->job)
		drm_capture_complete(f->job, -ECANCELED);
	drm_capture_close(f->capture);
}

static struct capture_fixture *new_fixture(struct kunit *test, bool claim)
{
	struct capture_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct drm_capture_job *job;

	KUNIT_ASSERT_NOT_NULL(test, f);
	f->capture = drm_capture_create(1, 16);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->capture);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, free_fixture, f), 0);
	KUNIT_ASSERT_EQ(test, drm_capture_queue(f->capture, &f->id), 0);
	if (claim) {
		job = drm_capture_claim(f->capture);
		KUNIT_ASSERT_NOT_ERR_OR_NULL(test, job);
		f->job = job;
	}
	return f;
}

struct capture_wait {
	struct capture_fixture *fixture;
	struct completion started, done;
	struct drm_capture_result result;
	int error;
};

static int wait_worker(void *data)
{
	struct capture_wait *wait = data;

	allow_signal(SIGUSR1);
	complete(&wait->started);
	wait->error = drm_capture_wait_result(wait->fixture->capture,
					      wait->fixture->id, &wait->result);
	flush_signals(current);
	complete(&wait->done);
	while (!kthread_should_stop())
		schedule_timeout_interruptible(1);
	return 0;
}

static struct task_struct *start_wait(struct kunit *test, struct capture_fixture *f,
				     struct capture_wait *wait)
{
	struct task_struct *worker;
	unsigned int attempts = 100;

	wait->fixture = f;
	wait->result.completed = false;
	wait->result.status = 123;
	init_completion(&wait->started);
	init_completion(&wait->done);
	worker = kthread_run(wait_worker, wait, "capture-result-wait");
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, worker);
	wait_for_completion(&wait->started);
	while (!waitqueue_active(drm_capture_result_waitqueue(f->capture)) && attempts--)
		schedule_timeout_uninterruptible(1);
	KUNIT_EXPECT_TRUE(test, waitqueue_active(drm_capture_result_waitqueue(f->capture)));
	KUNIT_EXPECT_FALSE(test, completion_done(&wait->done));
	return worker;
}

static void join_wait(struct kunit *test, struct task_struct *worker,
		      struct capture_wait *wait, int error, int status)
{
	unsigned long finished = wait_for_completion_timeout(&wait->done, HZ);

	if (!finished)
		send_sig(SIGUSR1, worker, 0);
	kthread_stop(worker);
	KUNIT_EXPECT_NE(test, finished, 0);
	KUNIT_EXPECT_EQ(test, wait->error, error);
	KUNIT_EXPECT_EQ(test, wait->result.completed, error == 0);
	KUNIT_EXPECT_EQ(test, wait->result.status, error ? 123 : status);
}

static void completion_wakes_without_consuming_result(struct kunit *test)
{
	struct capture_fixture *f = new_fixture(test, true);
	struct capture_wait wait;
	struct task_struct *worker = start_wait(test, f, &wait);
	u8 output[16] = {};

	memset(drm_capture_job_data(f->job), 0x35, 16);
	drm_capture_complete(f->job, 0);
	f->job = NULL;
	join_wait(test, worker, &wait, 0, 0);
	KUNIT_EXPECT_EQ(test, drm_capture_copy_result(f->capture, f->id, output, sizeof(output)), 16);
	KUNIT_EXPECT_PTR_EQ(test, memchr_inv(output, 0x35, sizeof(output)), NULL);
	KUNIT_EXPECT_EQ(test, drm_capture_wait_result(f->capture, f->id, &wait.result), 0);
}

static void producer_error_is_not_a_wait_error(struct kunit *test)
{
	struct capture_fixture *f = new_fixture(test, true);
	struct capture_wait wait;
	struct task_struct *worker = start_wait(test, f, &wait);

	drm_capture_complete(f->job, -EIO);
	f->job = NULL;
	join_wait(test, worker, &wait, 0, -EIO);
}

static void queued_cancellation_wakes_wait(struct kunit *test)
{
	struct capture_fixture *f = new_fixture(test, false);
	struct capture_wait wait;
	struct task_struct *worker = start_wait(test, f, &wait);

	KUNIT_EXPECT_EQ(test, drm_capture_cancel(f->capture, f->id), 0);
	join_wait(test, worker, &wait, 0, -ECANCELED);
}

static void revoked_claim_waits_for_actual_completion(struct kunit *test)
{
	struct capture_fixture *f = new_fixture(test, true);
	struct capture_wait wait;
	struct task_struct *worker = start_wait(test, f, &wait);

	drm_capture_revoke(f->capture);
	schedule_timeout_uninterruptible(msecs_to_jiffies(20));
	KUNIT_EXPECT_FALSE(test, completion_done(&wait.done));
	memset(drm_capture_job_data(f->job), 0x77, 16);
	drm_capture_complete(f->job, 0);
	f->job = NULL;
	join_wait(test, worker, &wait, 0, -EKEYREVOKED);
}

static void queued_revocation_wakes_wait(struct kunit *test)
{
	struct capture_fixture *f = new_fixture(test, false);
	struct capture_wait wait;
	struct task_struct *worker = start_wait(test, f, &wait);

	drm_capture_revoke(f->capture);
	join_wait(test, worker, &wait, 0, -EKEYREVOKED);
}

static void canceled_claim_waits_for_actual_completion(struct kunit *test)
{
	struct capture_fixture *f = new_fixture(test, true);
	struct capture_wait wait;
	struct task_struct *worker = start_wait(test, f, &wait);

	KUNIT_EXPECT_EQ(test, drm_capture_cancel(f->capture, f->id), 0);
	schedule_timeout_uninterruptible(msecs_to_jiffies(20));
	KUNIT_EXPECT_FALSE(test, completion_done(&wait.done));
	drm_capture_complete(f->job, 0);
	f->job = NULL;
	join_wait(test, worker, &wait, 0, -ECANCELED);
}

static void discard_wakes_without_releasing_provider_storage(struct kunit *test)
{
	struct capture_fixture *f = new_fixture(test, true);
	struct capture_wait wait;
	struct task_struct *worker = start_wait(test, f, &wait);
	u64 next;

	KUNIT_EXPECT_EQ(test, drm_capture_discard(f->capture, f->id), 0);
	join_wait(test, worker, &wait, -ENOENT, 0);
	KUNIT_EXPECT_EQ(test, drm_capture_queue(f->capture, &next), -EAGAIN);
	memset(drm_capture_job_data(f->job), 0x77, 16);
}

static void shutdown_wakes_removed_request(struct kunit *test)
{
	struct capture_fixture *f = new_fixture(test, false);
	struct capture_wait wait;
	struct task_struct *worker = start_wait(test, f, &wait);

	drm_capture_shutdown(f->capture);
	join_wait(test, worker, &wait, -ENOENT, 0);
}

static void interrupted_wait_leaves_request_pending(struct kunit *test)
{
	struct capture_fixture *f = new_fixture(test, false);
	struct capture_wait wait;
	struct task_struct *worker = start_wait(test, f, &wait);
	struct drm_capture_result result;

	send_sig(SIGUSR1, worker, 0);
	join_wait(test, worker, &wait, -ERESTARTSYS, 0);
	KUNIT_ASSERT_EQ(test, drm_capture_query(f->capture, f->id, &result), 0);
	KUNIT_EXPECT_FALSE(test, result.completed);
}

static struct kunit_case capture_wait_cases[] = {
	KUNIT_CASE(completion_wakes_without_consuming_result),
	KUNIT_CASE(producer_error_is_not_a_wait_error),
	KUNIT_CASE(queued_cancellation_wakes_wait),
	KUNIT_CASE(revoked_claim_waits_for_actual_completion),
	KUNIT_CASE(queued_revocation_wakes_wait),
	KUNIT_CASE(canceled_claim_waits_for_actual_completion),
	KUNIT_CASE(discard_wakes_without_releasing_provider_storage),
	KUNIT_CASE(shutdown_wakes_removed_request),
	KUNIT_CASE(interrupted_wait_leaves_request_pending),
	{}
};

static struct kunit_suite capture_wait_suite = {
	.name = "drm_capture_wait",
	.test_cases = capture_wait_cases,
};
kunit_test_suite(capture_wait_suite);
MODULE_LICENSE("GPL");
