// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <drm/drm_atomic.h>
#include <drm/drm_atomic_helper.h>
#include <drm/drm_atomic_prepare.h>
#include <drm/drm_atomic_prepare_display.h>
#include <drm/drm_atomic_prepare_owner.h>
#include <drm/drm_atomic_prepare_request.h>
#include <drm/drm_kunit_helpers.h>
#include <kunit/test.h>
#include <linux/completion.h>
#include <linux/kthread.h>

struct owner_request_fixture {
	struct drm_device *dev;
	struct drm_crtc *crtc;
	struct drm_prepare_owner *owner;
	struct drm_prepare_source *source;
	struct drm_prepare_read_claim *read;
	struct task_struct *worker;
	struct completion checked;
	unsigned int builds;
	unsigned int checks;
	unsigned int installs;
	int worker_error;
};

static int check_request(struct drm_device *dev, struct drm_atomic_commit *state)
{
	struct owner_request_fixture *f = dev->dev_private;
	int ret = drm_atomic_helper_check(dev, state);

	f->checks++;
	complete_all(&f->checked);
	return ret;
}

static int install_request(struct drm_device *dev, struct drm_atomic_commit *state,
			   bool nonblock)
{
	struct owner_request_fixture *f = dev->dev_private;
	int ret;

	ret = drm_atomic_helper_swap_state(state, false);
	if (!ret)
		f->installs++;
	return ret;
}

static const struct drm_mode_config_funcs request_funcs = {
	.atomic_check = check_request,
	.atomic_commit = install_request,
};

static void finish_fixture(void *data)
{
	struct owner_request_fixture *f = data;

	complete_all(&f->checked);
	if (f->worker)
		kthread_stop(f->worker);
	if (f->read)
		drm_prepare_read_abandon(f->read);
	if (f->source)
		drm_prepare_source_put(f->source);
	drm_prepare_owner_revoke(f->owner);
	drm_prepare_owner_put(f->owner);
}

static struct owner_request_fixture *new_request(struct kunit *test)
{
	struct owner_request_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct device *parent = drm_kunit_helper_alloc_device(test);
	struct drm_plane *plane;

	KUNIT_ASSERT_NOT_NULL(test, f);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	f->dev = __drm_kunit_helper_alloc_drm_device(test, parent, sizeof(*f->dev), 0,
						  DRIVER_MODESET | DRIVER_ATOMIC);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->dev);
	f->dev->mode_config.funcs = &request_funcs;
	f->dev->dev_private = f;
	KUNIT_ASSERT_EQ(test, drm_atomic_prepare_display_init(f->dev, 8), 0);
	plane = drm_kunit_helper_create_primary_plane(test, f->dev, NULL, NULL, NULL, 0, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, plane);
	f->crtc = drm_kunit_helper_create_crtc(test, f->dev, plane, NULL, NULL, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->crtc);
	drm_mode_config_reset(f->dev);
	init_completion(&f->checked);
	f->owner = drm_prepare_owner_create(2);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->owner);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, finish_fixture, f), 0);
	return f;
}

static int build_request(struct drm_atomic_commit *state, void *data)
{
	struct owner_request_fixture *f = data;
	struct drm_crtc_state *crtc = drm_atomic_get_crtc_state(state, f->crtc);

	f->builds++;
	return PTR_ERR_OR_ZERO(crtc);
}

static int finish_reader(void *data)
{
	struct owner_request_fixture *f = data;
	struct drm_modeset_acquire_ctx ctx;

	wait_for_completion(&f->checked);
	drm_modeset_acquire_init(&ctx, 0);
	f->worker_error = drm_modeset_lock(&f->crtc->mutex, &ctx);
	drm_prepare_read_release(f->read, NULL);
	f->read = NULL;
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

static void start_reader(struct kunit *test, struct owner_request_fixture *f)
{
	struct drm_prepare_source *source;
	struct drm_prepare_read_claim *read = NULL;
	struct task_struct *worker;

	KUNIT_ASSERT_EQ(test, drm_modeset_lock(&f->crtc->mutex, NULL), 0);
	source = drm_atomic_prepare_crtc_source(f->crtc);
	if (!IS_ERR(source)) {
		f->source = drm_prepare_source_get(source);
		read = drm_prepare_source_claim(source);
	}
	drm_modeset_unlock(&f->crtc->mutex);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, source);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, read);
	f->read = read;
	worker = kthread_run(finish_reader, f, "drm-owner-reader");
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, worker);
	f->worker = worker;
}

static void join_reader(struct owner_request_fixture *f)
{
	kthread_stop(f->worker);
	f->worker = NULL;
}

static void owned_request_rebuilds_after_reader_release(struct kunit *test)
{
	struct owner_request_fixture *f = new_request(test);

	start_reader(test, f);
	KUNIT_EXPECT_EQ(test, drm_atomic_commit_request_owned(f->dev, f->owner,
							   build_request, f), 0);
	join_reader(f);
	KUNIT_EXPECT_EQ(test, f->worker_error, 0);
	KUNIT_EXPECT_EQ(test, f->builds, 2);
	KUNIT_EXPECT_EQ(test, f->installs, 1);
}

static struct kunit_case cases[] = {
	KUNIT_CASE(owned_request_rebuilds_after_reader_release),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_prepare_request_owner",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_LICENSE("GPL");
