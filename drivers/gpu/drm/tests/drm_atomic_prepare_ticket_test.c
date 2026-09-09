// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/completion.h>
#include <linux/err.h>
#include <linux/kthread.h>
#include <linux/module.h>
#include <linux/sched.h>
#include <linux/sched/signal.h>
#include <drm/drm_atomic_prepare.h>
#include <drm/drm_atomic_prepare_ticket.h>
#include <kunit/test.h>

struct ticket_fixture {
	struct drm_prepare_source *source;
	struct drm_prepare_ticket *ticket;
	struct drm_prepare_read_claim *read;
};

static void free_fixture(void *data)
{
	struct ticket_fixture *f = data;

	if (f->read)
		drm_prepare_read_abandon(f->read);
	if (f->ticket)
		drm_prepare_ticket_put(f->ticket);
	if (f->source)
		drm_prepare_source_put(f->source);
}

static struct ticket_fixture *new_fixture_full(struct kunit *test, bool pending)
{
	struct ticket_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct drm_prepare_retirement_set *set;

	KUNIT_ASSERT_NOT_NULL(test, f);
	f->source = drm_prepare_source_create(1);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->source);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, free_fixture, f), 0);
	if (pending) {
		struct drm_prepare_read_claim *read = drm_prepare_source_claim(f->source);

		KUNIT_ASSERT_NOT_ERR_OR_NULL(test, read);
		f->read = read;
	}
	set = drm_prepare_retirement_set_create(&f->source, 1);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, set);
	f->ticket = drm_prepare_ticket_create(set);
	drm_prepare_retirement_set_put(set);
	if (IS_ERR(f->ticket)) {
		int error = PTR_ERR(f->ticket);

		f->ticket = NULL;
		KUNIT_FAIL(test, "ticket creation failed: %d", error);
		return NULL;
	}
	return f;
}

static struct ticket_fixture *new_fixture(struct kunit *test)
{
	struct ticket_fixture *f = new_fixture_full(test, false);

	KUNIT_ASSERT_NOT_NULL(test, f);
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

struct ticket_wait {
	struct drm_prepare_ticket *ticket;
	struct completion started;
	struct completion done;
	int result;
};

static int wait_for_ticket(void *data)
{
	struct ticket_wait *wait = data;

	allow_signal(SIGUSR1);
	complete(&wait->started);
	wait->result = drm_prepare_ticket_wait(wait->ticket);
	flush_signals(current);
	complete(&wait->done);
	while (!kthread_should_stop())
		schedule_timeout_interruptible(1);
	return 0;
}

static struct task_struct *start_ticket_wait(struct kunit *test, struct ticket_fixture *f,
					     struct ticket_wait *wait)
{
	struct task_struct *worker;

	wait->ticket = f->ticket;
	init_completion(&wait->started);
	init_completion(&wait->done);
	worker = kthread_run(wait_for_ticket, wait, "prepare-ticket-wait");
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, worker);
	wait_for_completion(&wait->started);
	schedule_timeout_uninterruptible(msecs_to_jiffies(20));
	KUNIT_EXPECT_FALSE(test, completion_done(&wait->done));
	return worker;
}

static void join_ticket_wait(struct kunit *test, struct task_struct *worker,
			     struct ticket_wait *wait, int expected)
{
	unsigned long finished = wait_for_completion_timeout(&wait->done, HZ);

	if (!finished)
		send_sig(SIGUSR1, worker, 0);
	kthread_stop(worker);
	KUNIT_EXPECT_NE(test, finished, 0);
	KUNIT_EXPECT_EQ(test, wait->result, expected);
}

static void cancellation_wakes_wait_without_resolving_claim(struct kunit *test)
{
	struct ticket_fixture *f = new_fixture_full(test, true);
	struct task_struct *worker;
	struct ticket_wait wait;

	KUNIT_ASSERT_NOT_NULL(test, f);
	worker = start_ticket_wait(test, f, &wait);
	drm_prepare_ticket_cancel(f->ticket);
	join_ticket_wait(test, worker, &wait, -ECANCELED);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_wait(f->ticket), -ECANCELED);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_source_claim(f->source)), -EAGAIN);
	drm_prepare_read_release(f->read, NULL);
	f->read = NULL;
	expect_open(test, f);
}

static void readiness_wait_does_not_reserve_the_ticket(struct kunit *test)
{
	struct ticket_fixture *f = new_fixture_full(test, true);
	struct drm_prepare_attempt *attempt;
	struct task_struct *worker;
	struct ticket_wait wait;

	KUNIT_ASSERT_NOT_NULL(test, f);
	worker = start_ticket_wait(test, f, &wait);
	drm_prepare_read_release(f->read, NULL);
	f->read = NULL;
	join_ticket_wait(test, worker, &wait, 0);
	attempt = reserve(test, f);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_wait(f->ticket), 0);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_ticket_reserve(f->ticket)), -EBUSY);
	kunit_release_action(test, destroy_attempt, attempt);
}

static void interrupted_ticket_wait_keeps_request_live(struct kunit *test)
{
	struct ticket_fixture *f = new_fixture_full(test, true);
	struct task_struct *worker;
	struct ticket_wait wait;

	KUNIT_ASSERT_NOT_NULL(test, f);
	worker = start_ticket_wait(test, f, &wait);
	send_sig(SIGUSR1, worker, 0);
	join_ticket_wait(test, worker, &wait, -ERESTARTSYS);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_source_claim(f->source)), -EBUSY);
	drm_prepare_read_release(f->read, NULL);
	f->read = NULL;
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_wait(f->ticket), 0);
}

static void ticket_wait_observes_consumption_and_empty_scope(struct kunit *test)
{
	struct ticket_fixture *f = new_fixture(test);
	struct drm_prepare_retirement_set *empty;
	struct drm_prepare_ticket *ticket;
	struct drm_prepare_attempt *attempt = reserve(test, f);
	struct drm_prepare_retirement_guard *guard;
	unsigned int installed = 0;

	KUNIT_ASSERT_EQ(test, drm_prepare_attempt_commit(attempt, count_install, &installed,
						       &guard), 0);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_wait(f->ticket), -EALREADY);
	drm_prepare_retirement_guard_destroy(guard);
	empty = drm_prepare_retirement_set_create(NULL, 0);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, empty);
	ticket = drm_prepare_ticket_create(empty);
	drm_prepare_retirement_set_put(empty);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, ticket);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_wait(ticket), 0);
	drm_prepare_ticket_cancel(ticket);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_wait(ticket), -ECANCELED);
	drm_prepare_ticket_put(ticket);
}

static void put_ticket(void *ticket)
{
	drm_prepare_ticket_put(ticket);
}

static void cancellation_does_not_cancel_another_ticket_in_the_domain(struct kunit *test)
{
	struct ticket_fixture *f = new_fixture_full(test, true);
	struct ticket_fixture peer = {};
	struct drm_prepare_retirement_set *set;
	struct task_struct *worker;
	struct ticket_wait wait;

	KUNIT_ASSERT_NOT_NULL(test, f);
	set = drm_prepare_retirement_set_create(&f->source, 1);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, set);
	peer.ticket = drm_prepare_ticket_create(set);
	drm_prepare_retirement_set_put(set);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, peer.ticket);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_ticket, peer.ticket), 0);
	worker = start_ticket_wait(test, &peer, &wait);
	drm_prepare_ticket_cancel(f->ticket);
	schedule_timeout_uninterruptible(msecs_to_jiffies(20));
	KUNIT_EXPECT_FALSE(test, completion_done(&wait.done));
	drm_prepare_read_release(f->read, NULL);
	f->read = NULL;
	join_ticket_wait(test, worker, &wait, 0);
}

static int record_notification(wait_queue_entry_t *entry, unsigned int mode, int flags, void *key)
{
	unsigned int *notifications = entry->private;

	++*notifications;
	return 1;
}

static void notification_outlives_canceled_source_ownership(struct kunit *test)
{
	struct ticket_fixture *f = new_fixture(test);
	wait_queue_head_t *queue = drm_prepare_ticket_waitqueue(f->ticket);
	wait_queue_entry_t entry;
	unsigned int notifications = 0;

	init_waitqueue_func_entry(&entry, record_notification);
	entry.private = &notifications;
	add_wait_queue(queue, &entry);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_ready(f->ticket), 0);
	drm_prepare_ticket_cancel(f->ticket);
	drm_prepare_source_put(f->source);
	f->source = NULL;
	KUNIT_EXPECT_PTR_EQ(test, drm_prepare_ticket_waitqueue(f->ticket), queue);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_ready(f->ticket), -ECANCELED);
	drm_prepare_ticket_cancel(f->ticket);
	KUNIT_EXPECT_GE(test, notifications, 1);
	remove_wait_queue(queue, &entry);
}

static void registered_observer_receives_claim_release(struct kunit *test)
{
	struct ticket_fixture *f = new_fixture_full(test, true);
	wait_queue_head_t *queue;
	wait_queue_entry_t entry;
	unsigned int notifications = 0;

	KUNIT_ASSERT_NOT_NULL(test, f);
	queue = drm_prepare_ticket_waitqueue(f->ticket);
	init_waitqueue_func_entry(&entry, record_notification);
	entry.private = &notifications;
	add_wait_queue(queue, &entry);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_ready(f->ticket), -EAGAIN);
	drm_prepare_read_release(f->read, NULL);
	f->read = NULL;
	KUNIT_EXPECT_GE(test, notifications, 1);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_ready(f->ticket), 0);
	remove_wait_queue(queue, &entry);
}

static void empty_ticket_notifies_terminal_state(struct kunit *test)
{
	struct drm_prepare_retirement_set *set = drm_prepare_retirement_set_create(NULL, 0);
	struct drm_prepare_ticket *ticket;
	wait_queue_head_t *queue;
	wait_queue_entry_t entry;
	unsigned int notifications = 0;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, set);
	ticket = drm_prepare_ticket_create(set);
	drm_prepare_retirement_set_put(set);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, ticket);
	queue = drm_prepare_ticket_waitqueue(ticket);
	init_waitqueue_func_entry(&entry, record_notification);
	entry.private = &notifications;
	add_wait_queue(queue, &entry);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_ready(ticket), 0);
	drm_prepare_ticket_cancel(ticket);
	KUNIT_EXPECT_GE(test, notifications, 1);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_ready(ticket), -ECANCELED);
	remove_wait_queue(queue, &entry);
	drm_prepare_ticket_put(ticket);
}

static struct kunit_case cases[] = {
	KUNIT_CASE(failed_attempt_leaves_ticket_retryable),
	KUNIT_CASE(cancellation_preserves_attempt_admission),
	KUNIT_CASE(acceptance_consumes_once),
	KUNIT_CASE(cancellation_races_one_installation_decision),
	KUNIT_CASE(cancellation_wakes_wait_without_resolving_claim),
	KUNIT_CASE(readiness_wait_does_not_reserve_the_ticket),
	KUNIT_CASE(interrupted_ticket_wait_keeps_request_live),
	KUNIT_CASE(ticket_wait_observes_consumption_and_empty_scope),
	KUNIT_CASE(cancellation_does_not_cancel_another_ticket_in_the_domain),
	KUNIT_CASE(notification_outlives_canceled_source_ownership),
	KUNIT_CASE(registered_observer_receives_claim_release),
	KUNIT_CASE(empty_ticket_notifies_terminal_state),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_prepare_ticket",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_LICENSE("GPL");
