// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <drm/drm_atomic.h>
#include <drm/drm_atomic_helper.h>
#include <drm/drm_atomic_prepare.h>
#include <drm/drm_atomic_prepare_display.h>
#include <drm/drm_atomic_prepare_request.h>
#include <drm/drm_kunit_helpers.h>
#include <kunit/test.h>
#include <linux/completion.h>
#include <linux/delay.h>
#include <linux/kthread.h>
#include <linux/sched/signal.h>

struct request_fixture {
	struct drm_device *dev;
	struct drm_crtc *crtc;
	struct drm_prepare_source *source;
	struct drm_prepare_read_claim *read;
	struct task_struct *worker;
	struct completion built;
	unsigned int builds;
	unsigned int checks;
	unsigned int installations;
	int build_error;
	int worker_error;
	bool replace;
	bool abandon;
	bool held_on_rebuild;
	bool fail_rebuild;
	struct task_struct *interrupt;
	u64 replacement_color;
	unsigned int signals_prepared;
	unsigned int signals_completed;
	unsigned int signals_accepted;
	bool signals_live;
	bool signaling_order_error;
	int signaling_error;
	int install_error;
};

static int check_request(struct drm_device *dev, struct drm_atomic_commit *state)
{
	struct request_fixture *f = dev->dev_private;
	int ret = drm_atomic_helper_check(dev, state);

	f->checks++;
	complete_all(&f->built);
	return ret;
}

static int install_request(struct drm_device *dev, struct drm_atomic_commit *state,
			   bool nonblock)
{
	struct request_fixture *f = dev->dev_private;
	int ret;

	if (f->install_error)
		return f->install_error;
	ret = drm_atomic_helper_swap_state(state, false);

	if (!ret)
		f->installations++;
	return ret;
}

static const struct drm_mode_config_funcs request_funcs = {
	.atomic_check = check_request,
	.atomic_commit = install_request,
};

static void stop_reader(void *data)
{
	struct request_fixture *f = data;

	complete_all(&f->built);
	if (f->worker)
		kthread_stop(f->worker);
	if (f->read)
		drm_prepare_read_abandon(f->read);
	if (f->source)
		drm_prepare_source_put(f->source);
}

static struct request_fixture *new_request(struct kunit *test, bool enabled)
{
	struct request_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct device *parent = drm_kunit_helper_alloc_device(test);
	struct drm_plane *plane;

	KUNIT_ASSERT_NOT_NULL(test, f);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	f->dev = __drm_kunit_helper_alloc_drm_device(test, parent, sizeof(*f->dev), 0,
						  DRIVER_MODESET | DRIVER_ATOMIC);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->dev);
	f->dev->mode_config.funcs = &request_funcs;
	f->dev->dev_private = f;
	if (enabled)
		KUNIT_ASSERT_EQ(test, drm_atomic_prepare_display_init(f->dev, 8), 0);
	plane = drm_kunit_helper_create_primary_plane(test, f->dev, NULL, NULL, NULL, 0, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, plane);
	f->crtc = drm_kunit_helper_create_crtc(test, f->dev, plane, NULL, NULL, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->crtc);
	drm_mode_config_reset(f->dev);
	init_completion(&f->built);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, stop_reader, f), 0);
	return f;
}

static int build_request(struct drm_atomic_commit *state, void *data)
{
	struct request_fixture *f = data;
	struct drm_crtc_state *crtc_state;
	struct drm_prepare_read_claim *read;

	crtc_state = drm_atomic_get_crtc_state(state, f->crtc);
	if (IS_ERR(crtc_state))
		return PTR_ERR(crtc_state);
	f->builds++;
	if (f->builds > 1 && f->source) {
		read = drm_prepare_source_claim(f->source);
		f->held_on_rebuild = IS_ERR(read) && PTR_ERR(read) == -EBUSY;
		if (!IS_ERR(read))
			drm_prepare_read_release(read, NULL);
	}
	if (f->builds > 1 && f->fail_rebuild)
		return -EACCES;
	return f->build_error;
}

static int finish_reader(void *data)
{
	struct request_fixture *f = data;
	struct drm_modeset_acquire_ctx ctx;
	struct drm_atomic_commit *state;
	unsigned long deadline;
	int ret;

	wait_for_completion(&f->built);
	drm_modeset_acquire_init(&ctx, 0);
	ctx.trylock_only = true;
	deadline = jiffies + HZ;
	do {
		ret = drm_modeset_lock(&f->crtc->mutex, &ctx);
		if (ret != -EBUSY)
			break;
		usleep_range(1000, 2000);
	} while (time_before(jiffies, deadline));
	if (!ret) {
		/* Trylock mode does not track locks or support nested acquisition. */
		drm_modeset_unlock(&f->crtc->mutex);
		ctx.trylock_only = false;
		ret = drm_modeset_lock(&f->crtc->mutex, &ctx);
	}
	f->worker_error = ret;
	if (!ret && f->replace) {
		state = drm_atomic_commit_alloc(f->dev);
		if (!state) {
			f->worker_error = -ENOMEM;
		} else {
			state->acquire_ctx = &ctx;
			if (IS_ERR(drm_atomic_get_crtc_state(state, f->crtc)))
				f->worker_error = -EINVAL;
			else {
				drm_atomic_get_new_crtc_state(state, f->crtc)->background_color =
					f->replacement_color;
				f->worker_error = drm_atomic_commit(state);
			}
			drm_atomic_commit_put(state);
		}
	}
	if (f->interrupt) {
		int signal_error = send_sig(SIGUSR1, f->interrupt, 0);

		if (!f->worker_error)
			f->worker_error = signal_error;
	} else if (f->abandon) {
		drm_prepare_read_abandon(f->read);
		f->read = NULL;
	} else {
		drm_prepare_read_release(f->read, NULL);
		f->read = NULL;
	}
	drm_modeset_drop_locks(&ctx);
	drm_modeset_acquire_fini(&ctx);
	set_current_state(TASK_INTERRUPTIBLE);
	while (!kthread_should_stop()) {
		schedule();
		set_current_state(TASK_INTERRUPTIBLE);
	}
	__set_current_state(TASK_RUNNING);
	return 0;
}

static void start_reader(struct kunit *test, struct request_fixture *f)
{
	struct drm_prepare_source *source;
	struct task_struct *worker;

	KUNIT_ASSERT_EQ(test, drm_modeset_lock(&f->crtc->mutex, NULL), 0);
	source = drm_atomic_prepare_crtc_source(f->crtc);
	if (!IS_ERR(source)) {
		f->source = drm_prepare_source_get(source);
		f->read = drm_prepare_source_claim(source);
	}
	drm_modeset_unlock(&f->crtc->mutex);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, source);
	if (IS_ERR(f->read)) {
		int ret = PTR_ERR(f->read);

		f->read = NULL;
		KUNIT_FAIL(test, "read claim failed: %d", ret);
		return;
	}
	worker = kthread_run(finish_reader, f, "drm-request-reader");
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, worker);
	f->worker = worker;
}

static void ready_request_installs_once(struct kunit *test)
{
	struct request_fixture *f = new_request(test, true);

	KUNIT_EXPECT_EQ(test, drm_atomic_commit_request(f->dev, build_request, f), 0);
	KUNIT_EXPECT_EQ(test, f->builds, 1);
	KUNIT_EXPECT_NOT_NULL(test, f->crtc->state->prepare_source);
}

static void ordinary_request_needs_no_accounting(struct kunit *test)
{
	struct request_fixture *f = new_request(test, false);

	KUNIT_EXPECT_EQ(test, drm_atomic_commit_request(f->dev, build_request, f), 0);
	KUNIT_EXPECT_EQ(test, f->builds, 1);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->prepare_source, NULL);
}

static void build_failure_does_not_install(struct kunit *test)
{
	struct request_fixture *f = new_request(test, true);
	struct drm_crtc_state *before = f->crtc->state;

	f->build_error = -EINVAL;
	KUNIT_EXPECT_EQ(test, drm_atomic_commit_request(f->dev, build_request, f), -EINVAL);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state, before);
	KUNIT_EXPECT_EQ(test, f->builds, 1);
}

static void unchanged_request_does_not_check_or_install(struct kunit *test)
{
	struct request_fixture *f = new_request(test, true);
	struct drm_crtc_state *before = f->crtc->state;

	f->build_error = DRM_ATOMIC_REQUEST_UNCHANGED;
	KUNIT_EXPECT_EQ(test, drm_atomic_commit_request(f->dev, build_request, f), 0);
	KUNIT_EXPECT_EQ(test, f->checks, 0);
	KUNIT_EXPECT_EQ(test, f->installations, 0);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state, before);
}

static void pending_reader_releases_locks_before_rebuild(struct kunit *test)
{
	struct request_fixture *f = new_request(test, true);

	start_reader(test, f);
	KUNIT_EXPECT_EQ(test, drm_atomic_commit_request(f->dev, build_request, f), 0);
	kthread_stop(f->worker);
	f->worker = NULL;
	KUNIT_EXPECT_EQ(test, f->worker_error, 0);
	KUNIT_EXPECT_EQ(test, f->builds, 2);
	KUNIT_EXPECT_TRUE(test, f->held_on_rebuild);
	KUNIT_EXPECT_PTR_NE(test, f->crtc->state->prepare_source, f->source);
}

static void replacement_during_wait_uses_current_generation(struct kunit *test)
{
	struct request_fixture *f = new_request(test, true);

	f->replace = true;
	start_reader(test, f);
	KUNIT_EXPECT_EQ(test, drm_atomic_commit_request(f->dev, build_request, f), 0);
	kthread_stop(f->worker);
	f->worker = NULL;
	KUNIT_EXPECT_EQ(test, f->worker_error, 0);
	KUNIT_EXPECT_EQ(test, f->builds, 2);
	KUNIT_EXPECT_TRUE(test, f->held_on_rebuild);
}

static void abandoned_reader_prevents_install(struct kunit *test)
{
	struct request_fixture *f = new_request(test, true);
	struct drm_crtc_state *before = f->crtc->state;

	f->abandon = true;
	start_reader(test, f);
	KUNIT_EXPECT_EQ(test, drm_atomic_commit_request(f->dev, build_request, f), -EIO);
	kthread_stop(f->worker);
	f->worker = NULL;
	KUNIT_EXPECT_EQ(test, f->worker_error, 0);
	KUNIT_EXPECT_EQ(test, f->builds, 1);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state, before);
}

static void rebuild_failure_releases_admission(struct kunit *test)
{
	struct request_fixture *f = new_request(test, true);
	struct drm_prepare_read_claim *read;

	f->fail_rebuild = true;
	start_reader(test, f);
	KUNIT_EXPECT_EQ(test, drm_atomic_commit_request(f->dev, build_request, f), -EACCES);
	kthread_stop(f->worker);
	f->worker = NULL;
	KUNIT_EXPECT_EQ(test, f->worker_error, 0);
	KUNIT_EXPECT_EQ(test, f->builds, 2);
	KUNIT_EXPECT_EQ(test, f->installations, 0);
	read = drm_prepare_source_claim(f->source);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, read);
	drm_prepare_read_release(read, NULL);
}

static void shutdown_waits_without_modeset_locks(struct kunit *test)
{
	struct request_fixture *f = new_request(test, true);

	start_reader(test, f);
	drm_atomic_helper_shutdown(f->dev);
	kthread_stop(f->worker);
	f->worker = NULL;
	KUNIT_EXPECT_EQ(test, f->worker_error, 0);
	KUNIT_EXPECT_EQ(test, f->checks, 2);
	KUNIT_EXPECT_EQ(test, f->installations, 1);
	KUNIT_EXPECT_FALSE(test, f->crtc->state->active);
	KUNIT_EXPECT_FALSE(test, f->crtc->state->enable);
	KUNIT_EXPECT_PTR_NE(test, f->crtc->state->prepare_source, f->source);
}

static void interrupted_request_preserves_unreleased_reader(struct kunit *test)
{
	struct request_fixture *f = new_request(test, true);
	struct drm_prepare_read_claim *read;
	int ret;

	f->interrupt = current;
	start_reader(test, f);
	allow_signal(SIGUSR1);
	ret = drm_atomic_commit_request(f->dev, build_request, f);
	flush_signals(current);
	disallow_signal(SIGUSR1);
	kthread_stop(f->worker);
	f->worker = NULL;
	KUNIT_EXPECT_EQ(test, ret, -ERESTARTSYS);
	KUNIT_EXPECT_EQ(test, f->worker_error, 0);
	KUNIT_EXPECT_EQ(test, f->installations, 0);
	KUNIT_ASSERT_NOT_NULL(test, f->read);
	read = drm_prepare_source_claim(f->source);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, read);
	drm_prepare_read_release(read, NULL);
	drm_prepare_read_release(f->read, NULL);
	f->read = NULL;
	KUNIT_EXPECT_EQ(test, drm_atomic_commit_request(f->dev, build_request, f), 0);
	KUNIT_EXPECT_EQ(test, f->installations, 1);
}

static int prepare_request_signaling(struct drm_atomic_commit *state, void *data)
{
	struct request_fixture *f = data;

	f->signaling_order_error |= f->signals_live || f->read ||
		!drm_modeset_is_locked(&f->crtc->mutex) || f->checks != f->builds;
	f->signals_prepared++;
	f->signals_live = true;
	return f->signaling_error;
}

static void complete_request_signaling(struct drm_atomic_commit *state, bool accepted, void *data)
{
	struct request_fixture *f = data;

	f->signaling_order_error |= !f->signals_live || !drm_modeset_is_locked(&f->crtc->mutex);
	f->signals_completed++;
	f->signals_accepted += accepted;
	f->signals_live = false;
}

static const struct drm_atomic_request_callbacks signaling_callbacks = {
	.build = build_request,
	.prepare_signaling = prepare_request_signaling,
	.complete_signaling = complete_request_signaling,
};

static void signaling_waits_for_preparation(struct kunit *test)
{
	struct request_fixture *f = new_request(test, true);

	start_reader(test, f);
	KUNIT_EXPECT_EQ(test, drm_atomic_commit_request_with_callbacks(f->dev, NULL,
								     &signaling_callbacks, f), 0);
	kthread_stop(f->worker);
	f->worker = NULL;
	KUNIT_EXPECT_EQ(test, f->worker_error, 0);
	KUNIT_EXPECT_EQ(test, f->builds, 2);
	KUNIT_EXPECT_EQ(test, f->signals_prepared, 1);
	KUNIT_EXPECT_EQ(test, f->signals_completed, 1);
	KUNIT_EXPECT_EQ(test, f->signals_accepted, 1);
	KUNIT_EXPECT_FALSE(test, f->signals_live);
	KUNIT_EXPECT_FALSE(test, f->signaling_order_error);
}

static void failed_signaling_is_completed_without_installation(struct kunit *test)
{
	struct request_fixture *f = new_request(test, true);

	f->signaling_error = -ENOMEM;
	KUNIT_EXPECT_EQ(test, drm_atomic_commit_request_with_callbacks(f->dev, NULL,
								     &signaling_callbacks, f), -ENOMEM);
	KUNIT_EXPECT_EQ(test, f->installations, 0);
	KUNIT_EXPECT_EQ(test, f->signals_prepared, 1);
	KUNIT_EXPECT_EQ(test, f->signals_completed, 1);
	KUNIT_EXPECT_EQ(test, f->signals_accepted, 0);
	KUNIT_EXPECT_FALSE(test, f->signals_live);
	KUNIT_EXPECT_FALSE(test, f->signaling_order_error);
}

static void rejected_commit_does_not_publish_signaling(struct kunit *test)
{
	struct request_fixture *f = new_request(test, true);

	f->install_error = -EBUSY;
	KUNIT_EXPECT_EQ(test, drm_atomic_commit_request_with_callbacks(f->dev, NULL,
								     &signaling_callbacks, f), -EBUSY);
	KUNIT_EXPECT_EQ(test, f->installations, 0);
	KUNIT_EXPECT_EQ(test, f->signals_prepared, 1);
	KUNIT_EXPECT_EQ(test, f->signals_completed, 1);
	KUNIT_EXPECT_EQ(test, f->signals_accepted, 0);
	KUNIT_EXPECT_FALSE(test, f->signals_live);
	KUNIT_EXPECT_FALSE(test, f->signaling_order_error);
}

static void unchanged_request_does_not_prepare_signaling(struct kunit *test)
{
	struct request_fixture *f = new_request(test, true);

	f->build_error = DRM_ATOMIC_REQUEST_UNCHANGED;
	KUNIT_EXPECT_EQ(test, drm_atomic_commit_request_with_callbacks(f->dev, NULL,
								     &signaling_callbacks, f), 0);
	KUNIT_EXPECT_EQ(test, f->checks, 0);
	KUNIT_EXPECT_EQ(test, f->signals_prepared, 0);
	KUNIT_EXPECT_EQ(test, f->signals_completed, 0);
	KUNIT_EXPECT_EQ(test, f->installations, 0);
}

static void failed_preparation_does_not_prepare_signaling(struct kunit *test)
{
	struct request_fixture *f = new_request(test, true);

	f->abandon = true;
	start_reader(test, f);
	KUNIT_EXPECT_EQ(test, drm_atomic_commit_request_with_callbacks(f->dev, NULL,
								     &signaling_callbacks, f), -EIO);
	kthread_stop(f->worker);
	f->worker = NULL;
	KUNIT_EXPECT_EQ(test, f->worker_error, 0);
	KUNIT_EXPECT_EQ(test, f->signals_prepared, 0);
	KUNIT_EXPECT_EQ(test, f->signals_completed, 0);
	KUNIT_EXPECT_EQ(test, f->installations, 0);
}

static void signaling_callbacks_must_be_paired(struct kunit *test)
{
	struct request_fixture *f = new_request(test, true);
	struct drm_atomic_request_callbacks callbacks = signaling_callbacks;

	callbacks.complete_signaling = NULL;
	KUNIT_EXPECT_EQ(test, drm_atomic_commit_request_with_callbacks(f->dev, NULL, &callbacks, f),
			-EINVAL);
	callbacks = signaling_callbacks;
	callbacks.prepare_signaling = NULL;
	KUNIT_EXPECT_EQ(test, drm_atomic_commit_request_with_callbacks(f->dev, NULL, &callbacks, f),
			-EINVAL);
	KUNIT_EXPECT_EQ(test, f->builds, 0);
	KUNIT_EXPECT_EQ(test, f->signals_prepared, 0);
	KUNIT_EXPECT_EQ(test, f->signals_completed, 0);
}

struct contended_request {
	struct request_fixture *display;
	struct drm_crtc *other;
	struct task_struct *worker;
	struct completion locked;
	struct completion first_locked;
	int worker_error;
	unsigned int attempts;
	unsigned int deadlocks;
};

static int contend_request(void *data)
{
	struct contended_request *f = data;
	struct drm_modeset_acquire_ctx ctx;

	drm_modeset_acquire_init(&ctx, 0);
	for (;;) {
		f->worker_error = drm_modeset_lock(&f->other->mutex, &ctx);
		complete_all(&f->locked);
		if (!f->worker_error) {
			wait_for_completion(&f->first_locked);
			f->worker_error = drm_modeset_lock(&f->display->crtc->mutex, &ctx);
		}
		if (f->worker_error != -EDEADLK)
			break;
		f->worker_error = drm_modeset_backoff(&ctx);
		if (f->worker_error)
			break;
	}
	drm_modeset_drop_locks(&ctx);
	drm_modeset_acquire_fini(&ctx);
	set_current_state(TASK_INTERRUPTIBLE);
	while (!kthread_should_stop()) {
		schedule();
		set_current_state(TASK_INTERRUPTIBLE);
	}
	__set_current_state(TASK_RUNNING);
	return 0;
}

static void stop_contender(void *data)
{
	struct contended_request *f = data;

	complete_all(&f->first_locked);
	kthread_stop(f->worker);
}

static int build_contended_request(struct drm_atomic_commit *state, void *data)
{
	struct contended_request *f = data;
	struct drm_crtc_state *crtc_state;

	f->attempts++;
	crtc_state = drm_atomic_get_crtc_state(state, f->display->crtc);
	if (IS_ERR(crtc_state))
		return PTR_ERR(crtc_state);
	complete_all(&f->first_locked);
	crtc_state = drm_atomic_get_crtc_state(state, f->other);
	if (IS_ERR(crtc_state)) {
		if (PTR_ERR(crtc_state) == -EDEADLK)
			f->deadlocks++;
		return PTR_ERR(crtc_state);
	}
	return 0;
}

static void contended_request_rebuilds_after_real_deadlock(struct kunit *test)
{
	struct request_fixture *display = new_request(test, true);
	struct contended_request *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct drm_plane *plane;

	KUNIT_ASSERT_NOT_NULL(test, f);
	f->display = display;
	plane = drm_kunit_helper_create_primary_plane(test, display->dev,
						     NULL, NULL, NULL, 0, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, plane);
	f->other = drm_kunit_helper_create_crtc(test, display->dev, plane, NULL, NULL, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->other);
	drm_mode_config_reset(display->dev);
	init_completion(&f->locked);
	init_completion(&f->first_locked);
	f->worker = kthread_run(contend_request, f, "drm-request-contender");
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->worker);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, stop_contender, f), 0);
	KUNIT_ASSERT_NE(test, wait_for_completion_timeout(&f->locked, HZ), 0);
	KUNIT_ASSERT_EQ(test, f->worker_error, 0);
	KUNIT_EXPECT_EQ(test, drm_atomic_commit_request(display->dev,
						       build_contended_request, f), 0);
	KUNIT_EXPECT_GE(test, f->attempts, 2);
	KUNIT_EXPECT_GE(test, f->deadlocks, 1);
	KUNIT_EXPECT_EQ(test, display->installations, 1);
}

static void suspend_saves_the_state_disabled_after_wait(struct kunit *test)
{
	struct request_fixture *f = new_request(test, true);
	struct drm_atomic_commit *saved;
	struct drm_crtc_state *crtc_state;

	f->replace = true;
	f->replacement_color = 0xffff111122223333ULL;
	start_reader(test, f);
	saved = drm_atomic_helper_suspend(f->dev);
	kthread_stop(f->worker);
	f->worker = NULL;
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, saved);
	crtc_state = drm_atomic_get_new_crtc_state(saved, f->crtc);
	KUNIT_EXPECT_NOT_NULL(test, crtc_state);
	if (crtc_state)
		KUNIT_EXPECT_EQ(test, crtc_state->background_color, f->replacement_color);
	KUNIT_EXPECT_PTR_EQ(test, saved->acquire_ctx, NULL);
	KUNIT_EXPECT_EQ(test, f->worker_error, 0);
	KUNIT_EXPECT_EQ(test, f->installations, 2);
	KUNIT_EXPECT_EQ(test, drm_atomic_helper_resume(f->dev, saved), 0);
	KUNIT_EXPECT_EQ(test, f->crtc->state->background_color, f->replacement_color);
}

static void failed_suspend_returns_no_saved_state(struct kunit *test)
{
	struct request_fixture *f = new_request(test, true);
	struct drm_atomic_commit *saved;
	struct drm_crtc_state *before = f->crtc->state;

	f->abandon = true;
	start_reader(test, f);
	saved = drm_atomic_helper_suspend(f->dev);
	kthread_stop(f->worker);
	f->worker = NULL;
	KUNIT_EXPECT_TRUE(test, IS_ERR(saved));
	if (IS_ERR(saved))
		KUNIT_EXPECT_EQ(test, PTR_ERR(saved), -EIO);
	else
		drm_atomic_commit_put(saved);
	KUNIT_EXPECT_EQ(test, f->worker_error, 0);
	KUNIT_EXPECT_EQ(test, f->installations, 0);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state, before);
}

static struct kunit_case cases[] = {
	KUNIT_CASE(ready_request_installs_once),
	KUNIT_CASE(ordinary_request_needs_no_accounting),
	KUNIT_CASE(build_failure_does_not_install),
	KUNIT_CASE(unchanged_request_does_not_check_or_install),
	KUNIT_CASE(pending_reader_releases_locks_before_rebuild),
	KUNIT_CASE(replacement_during_wait_uses_current_generation),
	KUNIT_CASE(abandoned_reader_prevents_install),
	KUNIT_CASE(rebuild_failure_releases_admission),
	KUNIT_CASE(shutdown_waits_without_modeset_locks),
	KUNIT_CASE(interrupted_request_preserves_unreleased_reader),
	KUNIT_CASE(contended_request_rebuilds_after_real_deadlock),
	KUNIT_CASE(suspend_saves_the_state_disabled_after_wait),
	KUNIT_CASE(failed_suspend_returns_no_saved_state),
	KUNIT_CASE(signaling_waits_for_preparation),
	KUNIT_CASE(failed_signaling_is_completed_without_installation),
	KUNIT_CASE(rejected_commit_does_not_publish_signaling),
	KUNIT_CASE(unchanged_request_does_not_prepare_signaling),
	KUNIT_CASE(failed_preparation_does_not_prepare_signaling),
	KUNIT_CASE(signaling_callbacks_must_be_paired),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_prepare_request",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_LICENSE("GPL");
