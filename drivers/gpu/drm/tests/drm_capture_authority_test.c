// SPDX-License-Identifier: GPL-2.0-only

#include <linux/completion.h>
#include <linux/err.h>
#include <linux/kthread.h>
#include <linux/module.h>
#include <drm/drm_capture.h>
#include <drm/drm_capture_authority.h>
#include <kunit/test.h>

struct authority_context {
	struct drm_capture *stream;
	unsigned int revokes;
	unsigned int releases;
	bool block;
	struct completion entered;
	struct completion unblock;
};

static void authority_revoke(void *data)
{
	struct authority_context *context = data;

	context->revokes++;
	if (context->block) {
		complete(&context->entered);
		wait_for_completion(&context->unblock);
	}
	if (context->stream)
		drm_capture_revoke(context->stream);
}

static void authority_release(void *data)
{
	struct authority_context *context = data;

	context->releases++;
	if (context->stream)
		drm_capture_close(context->stream);
}

static const struct drm_capture_authority_ops authority_ops = {
	.owner = THIS_MODULE,
	.revoke = authority_revoke,
	.release = authority_release,
};

static void authority_put(void *authority)
{
	drm_capture_authority_put(authority);
}

static void capture_put(void *capture)
{
	drm_capture_put(capture);
}

static void capture_job_cancel(void *job)
{
	drm_capture_complete(job, -ECANCELED);
}

static struct drm_capture_authority *
authority_create(struct kunit *test, struct authority_context **context)
{
	struct drm_capture_authority *authority;

	*context = kunit_kzalloc(test, sizeof(**context), GFP_KERNEL);
	KUNIT_ASSERT_NOT_NULL(test, *context);
	init_completion(&(*context)->entered);
	init_completion(&(*context)->unblock);
	authority = drm_capture_authority_create(&authority_ops, *context);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, authority);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, authority_put, authority), 0);
	return authority;
}

static void drm_capture_authority_terminal_cleanup(struct kunit *test)
{
	struct authority_context *context;
	struct drm_capture_authority *authority = authority_create(test, &context);

	KUNIT_ASSERT_EQ(test, drm_capture_authority_begin(authority), 0);
	drm_capture_authority_end(authority);
	KUNIT_EXPECT_FALSE(test, drm_capture_authority_revoked(authority));
	KUNIT_EXPECT_FALSE(test, drm_capture_authority_cleanup_done(authority));
	drm_capture_authority_revoke(authority);
	drm_capture_authority_revoke(authority);
	KUNIT_EXPECT_TRUE(test, drm_capture_authority_revoked(authority));
	KUNIT_EXPECT_TRUE(test, drm_capture_authority_cleanup_done(authority));
	if (!drm_capture_authority_begin(authority)) {
		drm_capture_authority_end(authority);
		KUNIT_FAIL(test, "revoked authority admitted work");
	}
	KUNIT_EXPECT_EQ(test, context->revokes, 1);
	kunit_release_action(test, authority_put, authority);
	KUNIT_EXPECT_EQ(test, context->revokes, 1);
	KUNIT_EXPECT_EQ(test, context->releases, 1);
}

static void drm_capture_authority_survives_stream_replacement(struct kunit *test)
{
	struct authority_context *context;
	struct drm_capture_authority *authority = authority_create(test, &context);
	struct drm_capture *old, *replacement;
	struct drm_capture_job *job;
	u64 id;

	replacement = drm_capture_create(1, 16);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, replacement);
	context->stream = replacement;
	old = drm_capture_get(context->stream);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, capture_put, old), 0);
	KUNIT_ASSERT_EQ(test, drm_capture_queue(old, &id), 0);
	job = drm_capture_claim(old);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, job);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, capture_job_cancel, job), 0);
	KUNIT_ASSERT_EQ(test, drm_capture_authority_begin(authority), 0);
	replacement = drm_capture_create(1, 32);
	if (!IS_ERR(replacement)) {
		drm_capture_close(context->stream);
		context->stream = replacement;
	}
	drm_capture_authority_end(authority);
	if (IS_ERR(replacement)) {
		KUNIT_FAIL(test, "replacement allocation failed");
		return;
	}
	KUNIT_EXPECT_FALSE(test, drm_capture_authority_revoked(authority));
	KUNIT_EXPECT_EQ(test, drm_capture_queue(old, &id), -EKEYREVOKED);
	KUNIT_EXPECT_EQ(test, drm_capture_queue(context->stream, &id), 0);
	drm_capture_authority_revoke(authority);
	KUNIT_EXPECT_EQ(test, drm_capture_queue(context->stream, &id), -EKEYREVOKED);
	/* The old job's lifetime is independent of both the replacement and grant. */
	memset(drm_capture_job_data(job), 0x5a, drm_capture_job_size(job));
	kunit_remove_action(test, capture_job_cancel, job);
	drm_capture_complete(job, 0);
	kunit_release_action(test, capture_put, old);
}

struct revoke_thread {
	struct drm_capture_authority *authority;
	struct completion done;
};

static int revoke_thread_run(void *data)
{
	struct revoke_thread *thread = data;

	drm_capture_authority_revoke(thread->authority);
	complete(&thread->done);
	return 0;
}

static void drm_capture_authority_concurrent_revoke(struct kunit *test)
{
	struct authority_context *context;
	struct drm_capture_authority *authority = authority_create(test, &context);
	struct revoke_thread first = { .authority = authority };
	struct revoke_thread second = { .authority = authority };
	struct task_struct *worker, *waiter;
	unsigned long entered, finished;

	init_completion(&first.done);
	init_completion(&second.done);
	context->block = true;
	worker = kthread_run(revoke_thread_run, &first, "capture-revoke");
	if (IS_ERR(worker)) {
		context->block = false;
		KUNIT_FAIL(test, "cannot start cleanup worker");
		return;
	}
	entered = wait_for_completion_timeout(&context->entered, HZ);
	if (!entered) {
		complete_all(&context->unblock);
		kthread_stop(worker);
		KUNIT_FAIL(test, "cleanup did not start");
		return;
	}
	if (!drm_capture_authority_begin(authority)) {
		drm_capture_authority_end(authority);
		KUNIT_FAIL(test, "revoking authority admitted work");
	}
	KUNIT_EXPECT_FALSE(test, drm_capture_authority_cleanup_done(authority));
	waiter = kthread_run(revoke_thread_run, &second, "capture-revoke-wait");
	if (!IS_ERR(waiter)) {
		finished = wait_for_completion_timeout(&second.done, msecs_to_jiffies(20));
		KUNIT_EXPECT_EQ(test, finished, 0);
	}
	complete_all(&context->unblock);
	kthread_stop(worker);
	if (IS_ERR(waiter)) {
		KUNIT_FAIL(test, "cannot start second revoker");
		return;
	}
	kthread_stop(waiter);
	KUNIT_EXPECT_EQ(test, context->revokes, 1);
	KUNIT_EXPECT_TRUE(test, drm_capture_authority_cleanup_done(authority));
	KUNIT_EXPECT_TRUE(test, completion_done(&second.done));
}

static struct kunit_case drm_capture_authority_cases[] = {
	KUNIT_CASE(drm_capture_authority_terminal_cleanup),
	KUNIT_CASE(drm_capture_authority_survives_stream_replacement),
	KUNIT_CASE(drm_capture_authority_concurrent_revoke),
	{}
};

static struct kunit_suite drm_capture_authority_suite = {
	.name = "drm_capture_authority",
	.test_cases = drm_capture_authority_cases,
};

kunit_test_suite(drm_capture_authority_suite);
MODULE_LICENSE("GPL");
