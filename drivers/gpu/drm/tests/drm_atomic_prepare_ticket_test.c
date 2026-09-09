// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/completion.h>
#include <linux/err.h>
#include <linux/kthread.h>
#include <linux/module.h>
#include <linux/sched.h>
#include <drm/drm_atomic_prepare.h>
#include <drm/drm_atomic_prepare_ticket.h>
#include <kunit/test.h>

struct ticket_fixture {
	struct drm_prepare_source *source;
	struct drm_prepare_ticket *ticket;
};

static void free_fixture(void *data)
{
	struct ticket_fixture *f = data;

	drm_prepare_ticket_put(f->ticket);
	drm_prepare_source_put(f->source);
}

static struct ticket_fixture *new_fixture(struct kunit *test)
{
	struct ticket_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct drm_prepare_retirement_set *set;

	KUNIT_ASSERT_NOT_NULL(test, f);
	f->source = drm_prepare_source_create(1);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->source);
	set = drm_prepare_retirement_set_create(&f->source, 1);
	if (IS_ERR(set)) {
		drm_prepare_source_put(f->source);
		KUNIT_ASSERT_NOT_ERR_OR_NULL(test, set);
	}
	f->ticket = drm_prepare_ticket_create(set);
	drm_prepare_retirement_set_put(set);
	if (IS_ERR(f->ticket)) {
		drm_prepare_source_put(f->source);
		KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->ticket);
	}
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, free_fixture, f), 0);
	return f;
}

static void destroy_attempt(void *attempt)
{
	drm_prepare_attempt_destroy(attempt);
}

static struct drm_prepare_attempt *reserve(struct kunit *test, struct ticket_fixture *f)
{
	struct drm_prepare_attempt *attempt = drm_prepare_ticket_reserve(f->ticket);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, attempt);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, destroy_attempt, attempt), 0);
	return attempt;
}

static int reject_install(void *data)
{
	return -ESTALE;
}

static int count_install(void *data)
{
	unsigned int *count = data;

	++*count;
	return 0;
}

static void expect_open(struct kunit *test, struct ticket_fixture *f)
{
	struct drm_prepare_read_claim *read = drm_prepare_source_claim(f->source);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, read);
	drm_prepare_read_release(read, NULL);
}

static void failed_attempt_leaves_ticket_retryable(struct kunit *test)
{
	struct ticket_fixture *f = new_fixture(test);
	struct drm_prepare_attempt *attempt = reserve(test, f);
	struct drm_prepare_retirement_guard *guard = ERR_PTR(-ENOENT);
	unsigned int installed = 0;

	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_ticket_reserve(f->ticket)), -EBUSY);
	KUNIT_EXPECT_EQ(test, drm_prepare_attempt_commit(attempt, reject_install, NULL, &guard),
			-ESTALE);
	KUNIT_EXPECT_PTR_EQ(test, guard, ERR_PTR(-ENOENT));
	kunit_release_action(test, destroy_attempt, attempt);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_source_claim(f->source)), -EBUSY);
	attempt = reserve(test, f);
	KUNIT_ASSERT_EQ(test, drm_prepare_attempt_commit(attempt, count_install, &installed,
						       &guard), 0);
	KUNIT_EXPECT_EQ(test, installed, 1);
	kunit_release_action(test, destroy_attempt, attempt);
	drm_prepare_ticket_cancel(f->ticket);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_source_claim(f->source)), -EBUSY);
	drm_prepare_retirement_guard_destroy(guard);
	expect_open(test, f);
}

static void cancellation_preserves_attempt_admission(struct kunit *test)
{
	struct ticket_fixture *f = new_fixture(test);
	struct drm_prepare_attempt *attempt = reserve(test, f);
	struct drm_prepare_retirement_guard *guard = ERR_PTR(-ENOENT);
	unsigned int installed = 0;

	drm_prepare_ticket_cancel(f->ticket);
	drm_prepare_ticket_cancel(f->ticket);
	KUNIT_EXPECT_EQ(test, drm_prepare_attempt_commit(attempt, count_install, &installed,
						       &guard), -ECANCELED);
	KUNIT_EXPECT_EQ(test, installed, 0);
	KUNIT_EXPECT_PTR_EQ(test, guard, ERR_PTR(-ENOENT));
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_ticket_reserve(f->ticket)), -ECANCELED);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_source_claim(f->source)), -EBUSY);
	kunit_release_action(test, destroy_attempt, attempt);
	expect_open(test, f);
}

static void acceptance_consumes_once(struct kunit *test)
{
	struct ticket_fixture *f = new_fixture(test);
	struct drm_prepare_attempt *attempt = reserve(test, f);
	struct drm_prepare_retirement_guard *guard, *untouched = ERR_PTR(-ENOENT);
	unsigned int installed = 0;

	KUNIT_ASSERT_EQ(test, drm_prepare_attempt_commit(attempt, count_install, &installed,
						       &guard), 0);
	drm_prepare_ticket_cancel(f->ticket);
	KUNIT_EXPECT_EQ(test, drm_prepare_attempt_commit(attempt, count_install, &installed,
						       &untouched), -EALREADY);
	KUNIT_EXPECT_PTR_EQ(test, untouched, ERR_PTR(-ENOENT));
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_ticket_reserve(f->ticket)), -EALREADY);
	KUNIT_EXPECT_EQ(test, installed, 1);
	kunit_release_action(test, destroy_attempt, attempt);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_source_claim(f->source)), -EBUSY);
	drm_prepare_retirement_guard_destroy(guard);
	expect_open(test, f);
}

struct cancel_race {
	struct completion start;
	struct completion canceled;
	struct drm_prepare_ticket *ticket;
};

static int cancel_worker(void *data)
{
	struct cancel_race *race = data;

	wait_for_completion(&race->start);
	drm_prepare_ticket_cancel(race->ticket);
	complete(&race->canceled);
	while (!kthread_should_stop())
		schedule_timeout_interruptible(1);
	return 0;
}

static void cancellation_races_one_installation_decision(struct kunit *test)
{
	unsigned int i;

	for (i = 0; i < 32; i++) {
		struct ticket_fixture *f = new_fixture(test);
		struct drm_prepare_attempt *attempt = reserve(test, f);
		struct drm_prepare_retirement_guard *guard = ERR_PTR(-ENOENT);
		struct cancel_race race = { .ticket = f->ticket };
		struct task_struct *worker;
		unsigned int installed = 0;
		unsigned long canceled;
		int result;

		init_completion(&race.start);
		init_completion(&race.canceled);
		worker = kthread_run(cancel_worker, &race, "drm-ticket-cancel");
		KUNIT_ASSERT_NOT_ERR_OR_NULL(test, worker);
		complete(&race.start);
		result = drm_prepare_attempt_commit(attempt, count_install, &installed, &guard);
		canceled = wait_for_completion_timeout(&race.canceled, HZ);
		kthread_stop(worker);
		KUNIT_EXPECT_NE(test, canceled, 0);
		KUNIT_EXPECT_TRUE(test, result == 0 || result == -ECANCELED);
		KUNIT_EXPECT_EQ(test, installed, result == 0 ? 1 : 0);
		KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_source_claim(f->source)), -EBUSY);
		kunit_release_action(test, destroy_attempt, attempt);
		if (!result)
			drm_prepare_retirement_guard_destroy(guard);
		else
			KUNIT_EXPECT_PTR_EQ(test, guard, ERR_PTR(-ENOENT));
		expect_open(test, f);
	}
}

static struct kunit_case cases[] = {
	KUNIT_CASE(failed_attempt_leaves_ticket_retryable),
	KUNIT_CASE(cancellation_preserves_attempt_admission),
	KUNIT_CASE(acceptance_consumes_once),
	KUNIT_CASE(cancellation_races_one_installation_decision),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_prepare_ticket",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_LICENSE("GPL");
