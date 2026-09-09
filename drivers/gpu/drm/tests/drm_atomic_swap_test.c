// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/completion.h>
#include <linux/kthread.h>
#include <linux/module.h>
#include <linux/sched/signal.h>
#include <drm/drm_atomic.h>
#include <drm/drm_atomic_helper.h>
#include <drm/drm_colorop.h>
#include <drm/drm_device.h>
#include <kunit/test.h>

/* Isolated state-pointer installation, without driver callbacks or registration. */
struct swap_fixture {
	struct drm_device dev;
	struct drm_atomic_commit state;
	struct drm_crtc crtc;
	struct drm_connector connector;
	struct drm_plane plane;
	struct drm_colorop colorop;
	struct drm_private_obj private_obj;
	struct drm_crtc_state crtc_states[2];
	struct drm_connector_state connector_states[2];
	struct drm_plane_state plane_states[2];
	struct drm_colorop_state colorop_states[2];
	struct drm_private_state private_states[2];
	struct __drm_crtcs_state crtc_slot;
	struct __drm_connnectors_state connector_slot;
	struct __drm_planes_state plane_slot;
	struct __drm_colorops_state colorop_slot;
	struct __drm_private_objs_state private_slot;
	struct drm_crtc_commit predecessors[3];
	struct completion finished;
	bool stall;
	bool interrupt;
	int result;
};

static struct swap_fixture *new_fixture(struct kunit *test)
{
	struct swap_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	unsigned int i;

	KUNIT_ASSERT_NOT_NULL(test, f);
	f->dev.mode_config.num_crtc = 1;
	f->dev.mode_config.num_total_plane = 1;
	f->dev.mode_config.num_colorop = 1;
	raw_spin_lock_init(&f->dev.mode_config.panic_lock);
	f->state.dev = &f->dev;
	f->state.num_connector = 1;
	f->state.num_private_objs = 1;
	f->state.crtcs = &f->crtc_slot;
	f->state.connectors = &f->connector_slot;
	f->state.planes = &f->plane_slot;
	f->state.colorops = &f->colorop_slot;
	f->state.private_objs = &f->private_slot;
	f->crtc.state = &f->crtc_states[0];
	f->connector.state = &f->connector_states[0];
	f->plane.state = &f->plane_states[0];
	f->colorop.state = &f->colorop_states[0];
	f->private_obj.state = &f->private_states[0];
	f->crtc_slot.ptr = &f->crtc;
	f->crtc_slot.old_state = &f->crtc_states[0];
	f->crtc_slot.new_state = &f->crtc_states[1];
	f->crtc_slot.state_to_destroy = &f->crtc_states[1];
	f->connector_slot.ptr = &f->connector;
	f->connector_slot.old_state = &f->connector_states[0];
	f->connector_slot.new_state = &f->connector_states[1];
	f->connector_slot.state_to_destroy = &f->connector_states[1];
	f->plane_slot.ptr = &f->plane;
	f->plane_slot.old_state = &f->plane_states[0];
	f->plane_slot.new_state = &f->plane_states[1];
	f->plane_slot.state_to_destroy = &f->plane_states[1];
	f->colorop_slot.ptr = &f->colorop;
	f->colorop_slot.old_state = &f->colorop_states[0];
	f->colorop_slot.new_state = &f->colorop_states[1];
	f->colorop_slot.state = &f->colorop_states[1];
	f->private_slot.ptr = &f->private_obj;
	f->private_slot.old_state = &f->private_states[0];
	f->private_slot.new_state = &f->private_states[1];
	f->private_slot.state_to_destroy = &f->private_states[1];
	f->crtc_states[0].commit = &f->predecessors[0];
	f->connector_states[0].commit = &f->predecessors[1];
	f->plane_states[0].commit = &f->predecessors[2];
	f->crtc_states[1].state = &f->state;
	f->connector_states[1].state = &f->state;
	f->plane_states[1].state = &f->state;
	f->colorop_states[1].state = &f->state;
	f->private_states[1].state = &f->state;
	for (i = 0; i < ARRAY_SIZE(f->predecessors); i++)
		init_completion(&f->predecessors[i].hw_done);
	init_completion(&f->finished);
	f->stall = true;
	return f;
}

static int swap_worker(void *data)
{
	struct swap_fixture *f = data;

	if (f->interrupt) {
		allow_signal(SIGUSR1);
		f->result = send_sig(SIGUSR1, current, 0);
		if (f->result)
			goto finished;
	}
	f->result = drm_atomic_helper_swap_state(&f->state, f->stall);
finished:
	flush_signals(current);
	complete(&f->finished);
	while (!kthread_should_stop())
		schedule_timeout_interruptible(1);
	return 0;
}

static void run_swap(struct kunit *test, struct swap_fixture *f)
{
	struct task_struct *worker = kthread_run(swap_worker, f, "drm-swap-test");
	unsigned long finished;
	unsigned int i;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, worker);
	finished = wait_for_completion_timeout(&f->finished, HZ);
	/* Unblock an unexpected wait before joining and releasing fixture storage. */
	for (i = 0; i < ARRAY_SIZE(f->predecessors); i++)
		complete_all(&f->predecessors[i].hw_done);
	kthread_stop(worker);
	KUNIT_ASSERT_NE(test, finished, 0);
}

static void expect_installed(struct kunit *test, struct swap_fixture *f)
{
	KUNIT_EXPECT_PTR_EQ(test, f->crtc.state, &f->crtc_states[1]);
	KUNIT_EXPECT_PTR_EQ(test, f->connector.state, &f->connector_states[1]);
	KUNIT_EXPECT_PTR_EQ(test, f->plane.state, &f->plane_states[1]);
	KUNIT_EXPECT_PTR_EQ(test, f->colorop.state, &f->colorop_states[1]);
	KUNIT_EXPECT_PTR_EQ(test, f->colorop_slot.state, &f->colorop_states[0]);
	KUNIT_EXPECT_PTR_EQ(test, f->colorop_states[0].state, &f->state);
	KUNIT_EXPECT_PTR_EQ(test, f->colorop_states[1].state, NULL);
	KUNIT_EXPECT_PTR_EQ(test, f->private_obj.state, &f->private_states[1]);
	KUNIT_EXPECT_PTR_EQ(test, f->private_slot.state_to_destroy, &f->private_states[0]);
	KUNIT_EXPECT_PTR_EQ(test, f->private_states[0].state, &f->state);
	KUNIT_EXPECT_PTR_EQ(test, f->private_states[1].state, NULL);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc_slot.state_to_destroy, &f->crtc_states[0]);
	KUNIT_EXPECT_PTR_EQ(test, f->connector_slot.state_to_destroy, &f->connector_states[0]);
	KUNIT_EXPECT_PTR_EQ(test, f->plane_slot.state_to_destroy, &f->plane_states[0]);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc_states[0].state, &f->state);
	KUNIT_EXPECT_PTR_EQ(test, f->connector_states[0].state, &f->state);
	KUNIT_EXPECT_PTR_EQ(test, f->plane_states[0].state, &f->state);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc_states[1].state, NULL);
	KUNIT_EXPECT_PTR_EQ(test, f->connector_states[1].state, NULL);
	KUNIT_EXPECT_PTR_EQ(test, f->plane_states[1].state, NULL);
}

static void each_interrupted_predecessor_leaves_all_state_uninstalled(struct kunit *test)
{
	unsigned int blocked, i;

	for (blocked = 0; blocked < 3; blocked++) {
		struct swap_fixture *f = new_fixture(test);

		for (i = 0; i < ARRAY_SIZE(f->predecessors); i++) {
			if (i != blocked)
				complete_all(&f->predecessors[i].hw_done);
		}
		f->interrupt = true;
		run_swap(test, f);
		KUNIT_ASSERT_EQ(test, f->result, -ERESTARTSYS);
		KUNIT_EXPECT_PTR_EQ(test, f->crtc.state, &f->crtc_states[0]);
		KUNIT_EXPECT_PTR_EQ(test, f->connector.state, &f->connector_states[0]);
		KUNIT_EXPECT_PTR_EQ(test, f->plane.state, &f->plane_states[0]);
		KUNIT_EXPECT_PTR_EQ(test, f->colorop.state, &f->colorop_states[0]);
		KUNIT_EXPECT_PTR_EQ(test, f->colorop_slot.state, &f->colorop_states[1]);
		KUNIT_EXPECT_PTR_EQ(test, f->colorop_states[0].state, NULL);
		KUNIT_EXPECT_PTR_EQ(test, f->colorop_states[1].state, &f->state);
		KUNIT_EXPECT_PTR_EQ(test, f->private_obj.state, &f->private_states[0]);
		KUNIT_EXPECT_PTR_EQ(test, f->private_slot.state_to_destroy, &f->private_states[1]);
		KUNIT_EXPECT_PTR_EQ(test, f->private_states[0].state, NULL);
		KUNIT_EXPECT_PTR_EQ(test, f->private_states[1].state, &f->state);
		KUNIT_EXPECT_PTR_EQ(test, f->crtc_slot.state_to_destroy, &f->crtc_states[1]);
		KUNIT_EXPECT_PTR_EQ(test, f->connector_slot.state_to_destroy, &f->connector_states[1]);
		KUNIT_EXPECT_PTR_EQ(test, f->plane_slot.state_to_destroy, &f->plane_states[1]);
		KUNIT_EXPECT_PTR_EQ(test, f->crtc_states[0].state, NULL);
		KUNIT_EXPECT_PTR_EQ(test, f->connector_states[0].state, NULL);
		KUNIT_EXPECT_PTR_EQ(test, f->plane_states[0].state, NULL);
		KUNIT_EXPECT_PTR_EQ(test, f->crtc_states[1].state, &f->state);
		KUNIT_EXPECT_PTR_EQ(test, f->connector_states[1].state, &f->state);
		KUNIT_EXPECT_PTR_EQ(test, f->plane_states[1].state, &f->state);
		KUNIT_ASSERT_EQ(test, drm_atomic_helper_swap_state(&f->state, true), 0);
		expect_installed(test, f);
	}
}

static void completed_predecessors_allow_installation_with_a_signal(struct kunit *test)
{
	struct swap_fixture *f = new_fixture(test);
	unsigned int i;

	for (i = 0; i < ARRAY_SIZE(f->predecessors); i++)
		complete_all(&f->predecessors[i].hw_done);
	f->interrupt = true;
	run_swap(test, f);
	KUNIT_ASSERT_EQ(test, f->result, 0);
	expect_installed(test, f);
}

static void no_stall_skips_pending_predecessors(struct kunit *test)
{
	struct swap_fixture *f = new_fixture(test);

	f->stall = false;
	run_swap(test, f);
	KUNIT_ASSERT_EQ(test, f->result, 0);
	expect_installed(test, f);
}

static void absent_predecessors_need_no_wait(struct kunit *test)
{
	struct swap_fixture *f = new_fixture(test);

	f->crtc_states[0].commit = NULL;
	f->connector_states[0].commit = NULL;
	f->plane_states[0].commit = NULL;
	f->interrupt = true;
	run_swap(test, f);
	KUNIT_ASSERT_EQ(test, f->result, 0);
	expect_installed(test, f);
}

static struct kunit_case cases[] = {
	KUNIT_CASE(each_interrupted_predecessor_leaves_all_state_uninstalled),
	KUNIT_CASE(completed_predecessors_allow_installation_with_a_signal),
	KUNIT_CASE(no_stall_skips_pending_predecessors),
	KUNIT_CASE(absent_predecessors_need_no_wait),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_swap",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_LICENSE("GPL");
