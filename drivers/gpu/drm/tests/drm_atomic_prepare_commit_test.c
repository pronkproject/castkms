// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/completion.h>
#include <linux/dma-fence.h>
#include <linux/kthread.h>
#include <linux/module.h>
#include <drm/drm_atomic.h>
#include <drm/drm_atomic_helper.h>
#include <drm/drm_atomic_prepare.h>
#include <drm/drm_atomic_prepare_commit.h>
#include <drm/drm_atomic_prepare_scope.h>
#include <drm/drm_atomic_prepare_ticket.h>
#include <drm/drm_device.h>
#include <kunit/test.h>

struct prepare_commit_fixture {
	struct drm_device dev;
	struct drm_atomic_commit state;
	struct drm_prepare_source *source;
	struct drm_prepare_ticket *ticket;
	struct dma_fence *fence;
	struct completion started;
	struct completion finished;
	unsigned int cleared;
	bool admission_held_at_clear;
	bool completed_at_clear;
	bool clear_in_worker;
	struct drm_prepare_scope_entry observed;
	int observation_count;
	unsigned int observations;
};

static void clear_objects(struct drm_atomic_commit *state)
{
	struct prepare_commit_fixture *f = container_of(state, typeof(*f), state);
	struct drm_prepare_read_claim *read = drm_prepare_source_claim(f->source);

	f->cleared++;
	f->admission_held_at_clear = IS_ERR(read) && PTR_ERR(read) == -EBUSY;
	f->completed_at_clear = !f->fence || dma_fence_is_signaled(f->fence);
	if (!IS_ERR(read))
		drm_prepare_read_release(read, NULL);
}

static const struct drm_mode_config_funcs mode_config_funcs = {
	.atomic_state_clear = clear_objects,
};

static const char *fence_name(struct dma_fence *fence)
{
	return "drm-prepare-commit-test";
}

static const struct dma_fence_ops fence_ops = {
	.get_driver_name = fence_name,
	.get_timeline_name = fence_name,
};

static void free_fixture(void *data)
{
	struct prepare_commit_fixture *f = data;

	if (f->fence)
		dma_fence_signal(f->fence);
	drm_atomic_commit_clear(&f->state);
	if (f->ticket)
		drm_prepare_ticket_put(f->ticket);
	drm_prepare_source_put(f->source);
	dma_fence_put(f->fence);
}

static struct prepare_commit_fixture *new_fixture(struct kunit *test, bool submitted)
{
	struct prepare_commit_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct drm_prepare_retirement_set *set;
	struct drm_prepare_read_claim *read;

	KUNIT_ASSERT_NOT_NULL(test, f);
	f->state.dev = &f->dev;
	f->dev.mode_config.funcs = &mode_config_funcs;
	raw_spin_lock_init(&f->dev.mode_config.panic_lock);
	init_completion(&f->started);
	init_completion(&f->finished);
	f->source = drm_prepare_source_create(1);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->source);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, free_fixture, f), 0);
	if (submitted) {
		f->fence = kzalloc_obj(*f->fence);
		KUNIT_ASSERT_NOT_NULL(test, f->fence);
		dma_fence_init(f->fence, &fence_ops, NULL, dma_fence_context_alloc(1), 1);
		read = drm_prepare_source_claim(f->source);
		KUNIT_ASSERT_NOT_ERR_OR_NULL(test, read);
		drm_prepare_read_release(read, f->fence);
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

static void expect_open(struct kunit *test, struct prepare_commit_fixture *f)
{
	struct drm_prepare_read_claim *read = drm_prepare_source_claim(f->source);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, read);
	drm_prepare_read_release(read, NULL);
}

static void clearing_unaccepted_transaction_allows_retry(struct kunit *test)
{
	struct prepare_commit_fixture *f = new_fixture(test, false);

	KUNIT_ASSERT_NOT_NULL(test, f);
	KUNIT_ASSERT_EQ(test, drm_atomic_commit_prepare(&f->state, f->ticket), 0);
	KUNIT_EXPECT_EQ(test, drm_atomic_commit_prepare(&f->state, f->ticket), -EBUSY);
	drm_atomic_commit_clear(&f->state);
	KUNIT_EXPECT_PTR_EQ(test, f->state.preparation, NULL);
	KUNIT_EXPECT_TRUE(test, f->admission_held_at_clear);
	KUNIT_ASSERT_EQ(test, drm_atomic_commit_prepare(&f->state, f->ticket), 0);
	drm_prepare_ticket_cancel(f->ticket);
	KUNIT_EXPECT_EQ(test, drm_atomic_helper_swap_state(&f->state, false), -ECANCELED);
	drm_atomic_commit_clear(&f->state);
	KUNIT_EXPECT_TRUE(test, f->admission_held_at_clear);
	expect_open(test, f);
}

static void accepted_transaction_holds_admission_through_object_cleanup(struct kunit *test)
{
	struct prepare_commit_fixture *f = new_fixture(test, false);

	KUNIT_ASSERT_NOT_NULL(test, f);
	KUNIT_ASSERT_EQ(test, drm_atomic_commit_prepare(&f->state, f->ticket), 0);
	KUNIT_ASSERT_EQ(test, drm_atomic_helper_swap_state(&f->state, false), 0);
	KUNIT_EXPECT_EQ(test, drm_atomic_helper_swap_state(&f->state, false), -EALREADY);
	drm_prepare_ticket_cancel(f->ticket);
	drm_prepare_ticket_put(f->ticket);
	f->ticket = NULL;
	drm_atomic_commit_clear(&f->state);
	KUNIT_EXPECT_EQ(test, f->cleared, 1);
	KUNIT_EXPECT_TRUE(test, f->admission_held_at_clear);
	KUNIT_EXPECT_PTR_EQ(test, f->state.preparation, NULL);
	expect_open(test, f);
}

static int wait_worker(void *data)
{
	struct prepare_commit_fixture *f = data;

	complete(&f->started);
	if (f->clear_in_worker)
		drm_atomic_commit_clear(&f->state);
	else
		drm_atomic_helper_wait_for_dependencies(&f->state);
	complete(&f->finished);
	set_current_state(TASK_INTERRUPTIBLE);
	while (!kthread_should_stop()) {
		schedule();
		set_current_state(TASK_INTERRUPTIBLE);
	}
	__set_current_state(TASK_RUNNING);
	return 0;
}

static void check_native_wait(struct kunit *test, bool clear, bool error)
{
	struct prepare_commit_fixture *f = new_fixture(test, true);
	struct task_struct *worker;
	bool returned_early;

	KUNIT_ASSERT_NOT_NULL(test, f);
	f->clear_in_worker = clear;
	KUNIT_ASSERT_EQ(test, drm_atomic_commit_prepare(&f->state, f->ticket), 0);
	KUNIT_ASSERT_EQ(test, drm_atomic_helper_swap_state(&f->state, false), 0);
	worker = kthread_run(wait_worker, f, "drm-prepare-commit");
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, worker);
	wait_for_completion(&f->started);
	returned_early = wait_for_completion_timeout(&f->finished, msecs_to_jiffies(20));
	if (error)
		dma_fence_set_error(f->fence, -EIO);
	dma_fence_signal(f->fence);
	if (!returned_early)
		wait_for_completion(&f->finished);
	kthread_stop(worker);
	KUNIT_EXPECT_FALSE(test, returned_early);
	if (!clear) {
		KUNIT_EXPECT_EQ(test, f->cleared, 0);
		drm_atomic_commit_clear(&f->state);
	}
	KUNIT_EXPECT_EQ(test, f->cleared, 1);
	KUNIT_EXPECT_TRUE(test, f->completed_at_clear);
	KUNIT_EXPECT_TRUE(test, f->admission_held_at_clear);
	expect_open(test, f);
}

static void commit_dependencies_wait_for_submitted_readers(struct kunit *test)
{
	check_native_wait(test, false, false);
}

static void object_cleanup_waits_for_failed_native_completion(struct kunit *test)
{
	check_native_wait(test, true, true);
}

static void async_update_does_not_bypass_preparation(struct kunit *test)
{
	struct prepare_commit_fixture *f = new_fixture(test, false);

	KUNIT_ASSERT_NOT_NULL(test, f);
	f->state.async_update = true;
	KUNIT_EXPECT_EQ(test, drm_atomic_commit_prepare(&f->state, f->ticket), -EOPNOTSUPP);
	KUNIT_EXPECT_PTR_EQ(test, f->state.preparation, NULL);
	f->state.async_update = false;
	KUNIT_ASSERT_EQ(test, drm_atomic_commit_prepare(&f->state, f->ticket), 0);
	f->state.async_update = true;
	KUNIT_EXPECT_EQ(test, drm_atomic_helper_commit(&f->dev, &f->state, false), -EOPNOTSUPP);
	KUNIT_EXPECT_EQ(test, drm_atomic_helper_swap_state(&f->state, false), -EOPNOTSUPP);
	f->state.async_update = false;
	KUNIT_EXPECT_EQ(test, drm_atomic_helper_swap_state(&f->state, false), 0);
}

static int observe_scope(struct drm_atomic_commit *state,
			 struct drm_prepare_scope_entry *entries,
			 unsigned int capacity)
{
	struct prepare_commit_fixture *f = container_of(state, typeof(*f), state);

	f->observations++;
	if (capacity && f->observation_count == 1)
		entries[0] = f->observed;
	return f->observation_count;
}

static void observe_final_scope_before_acceptance(struct kunit *test)
{
	struct prepare_commit_fixture *f = new_fixture(test, false);
	struct drm_prepare_scope_entry entry;

	KUNIT_ASSERT_NOT_NULL(test, f);
	drm_prepare_ticket_put(f->ticket);
	entry = (struct drm_prepare_scope_entry) { .crtc_id = 1, .source = f->source };
	f->ticket = drm_prepare_ticket_create_scoped(&entry, 1);
	if (IS_ERR(f->ticket)) {
		f->ticket = NULL;
		KUNIT_FAIL(test, "scoped ticket allocation failed");
		return;
	}
	f->observed = entry;
	f->observation_count = 1;
	KUNIT_EXPECT_EQ(test, drm_atomic_commit_prepare_scoped(&f->state, f->ticket, NULL),
			-EINVAL);
	KUNIT_EXPECT_PTR_EQ(test, f->state.preparation, NULL);
	KUNIT_ASSERT_EQ(test, drm_atomic_commit_prepare_scoped(&f->state, f->ticket,
							    observe_scope), 0);
	KUNIT_EXPECT_EQ(test, f->observations, 0);
	f->observed.crtc_id = 2;
	KUNIT_EXPECT_EQ(test, drm_atomic_helper_swap_state(&f->state, false), -ESTALE);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_status(f->ticket), DRM_PREPARE_TICKET_READY);
	f->observation_count = -EACCES;
	KUNIT_EXPECT_EQ(test, drm_atomic_helper_swap_state(&f->state, false), -EACCES);
	f->observation_count = DRM_PREPARE_SCOPE_MAX_OUTPUTS + 1;
	KUNIT_EXPECT_EQ(test, drm_atomic_helper_swap_state(&f->state, false), -E2BIG);
	f->observation_count = 0;
	KUNIT_EXPECT_EQ(test, drm_atomic_helper_swap_state(&f->state, false), -ESTALE);
	f->observed = entry;
	f->observation_count = 1;
	KUNIT_EXPECT_EQ(test, drm_atomic_helper_swap_state(&f->state, false), 0);
	KUNIT_EXPECT_EQ(test, f->observations, 5);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_status(f->ticket), DRM_PREPARE_TICKET_CONSUMED);
	KUNIT_EXPECT_EQ(test, drm_atomic_helper_swap_state(&f->state, false), -EALREADY);
	KUNIT_EXPECT_EQ(test, f->observations, 5);
	drm_atomic_commit_clear(&f->state);
	KUNIT_EXPECT_TRUE(test, f->admission_held_at_clear);
	expect_open(test, f);
}

static void scope_observer_does_not_authorize_unscoped_ticket(struct kunit *test)
{
	struct prepare_commit_fixture *f = new_fixture(test, false);

	KUNIT_ASSERT_NOT_NULL(test, f);
	KUNIT_ASSERT_EQ(test, drm_atomic_commit_prepare_scoped(&f->state, f->ticket,
							    observe_scope), 0);
	KUNIT_EXPECT_EQ(test, drm_atomic_helper_swap_state(&f->state, false), -EINVAL);
	drm_atomic_commit_clear(&f->state);
	KUNIT_ASSERT_EQ(test, drm_atomic_commit_prepare(&f->state, f->ticket), 0);
	KUNIT_EXPECT_EQ(test, drm_atomic_helper_swap_state(&f->state, false), 0);
}

static struct kunit_case prepare_commit_cases[] = {
	KUNIT_CASE(clearing_unaccepted_transaction_allows_retry),
	KUNIT_CASE(accepted_transaction_holds_admission_through_object_cleanup),
	KUNIT_CASE(commit_dependencies_wait_for_submitted_readers),
	KUNIT_CASE(object_cleanup_waits_for_failed_native_completion),
	KUNIT_CASE(async_update_does_not_bypass_preparation),
	KUNIT_CASE(observe_final_scope_before_acceptance),
	KUNIT_CASE(scope_observer_does_not_authorize_unscoped_ticket),
	{}
};

static struct kunit_suite prepare_commit_suite = {
	.name = "drm_atomic_prepare_commit",
	.test_cases = prepare_commit_cases,
};

kunit_test_suite(prepare_commit_suite);

MODULE_LICENSE("GPL");
