// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <drm/drm_atomic.h>
#include <drm/drm_atomic_helper.h>
#include <drm/drm_atomic_prepare.h>
#include <drm/drm_atomic_prepare_display.h>
#include <drm/drm_client.h>
#include <drm/drm_kunit_helpers.h>
#include <kunit/test.h>
#include <linux/completion.h>
#include <linux/kthread.h>

struct client_fixture {
	struct drm_client_dev client;
	struct drm_mode_set modesets[2];
	struct drm_connector *connectors[1];
	struct drm_crtc *crtc;
	struct drm_prepare_source *source;
	struct drm_prepare_read_claim *read;
	struct task_struct *worker;
	struct completion checked;
	unsigned int checks;
	unsigned int installs;
	int worker_error;
	bool abandon;
};

static int check_client(struct drm_device *dev, struct drm_atomic_commit *state)
{
	struct client_fixture *f = dev->dev_private;
	int ret = drm_atomic_helper_check(dev, state);

	f->checks++;
	complete_all(&f->checked);
	return ret;
}

static int install_client(struct drm_device *dev, struct drm_atomic_commit *state,
			  bool nonblock)
{
	struct client_fixture *f = dev->dev_private;
	int ret = drm_atomic_helper_swap_state(state, false);

	if (!ret)
		f->installs++;
	return ret;
}

static const struct drm_mode_config_funcs client_funcs = {
	.atomic_check = check_client,
	.atomic_commit = install_client,
};

static void finish_fixture(void *data)
{
	struct client_fixture *f = data;

	complete_all(&f->checked);
	if (f->worker)
		kthread_stop(f->worker);
	if (f->read)
		drm_prepare_read_abandon(f->read);
	if (f->source)
		drm_prepare_source_put(f->source);
	mutex_destroy(&f->client.modeset_mutex);
}

static struct client_fixture *new_client(struct kunit *test)
{
	struct client_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct device *parent = drm_kunit_helper_alloc_device(test);
	struct drm_prepare_source *source;
	struct drm_prepare_read_claim *read = NULL;
	struct drm_device *dev;
	struct drm_plane *plane;

	KUNIT_ASSERT_NOT_NULL(test, f);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	dev = __drm_kunit_helper_alloc_drm_device(test, parent, sizeof(*dev), 0,
					       DRIVER_MODESET | DRIVER_ATOMIC);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, dev);
	dev->dev_private = f;
	dev->mode_config.funcs = &client_funcs;
	KUNIT_ASSERT_EQ(test, drm_atomic_prepare_display_init(dev, 8), 0);
	plane = drm_kunit_helper_create_primary_plane(test, dev, NULL, NULL, NULL, 0, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, plane);
	f->crtc = drm_kunit_helper_create_crtc(test, dev, plane, NULL, NULL, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->crtc);
	drm_mode_config_reset(dev);
	f->client.dev = dev;
	f->client.modesets = f->modesets;
	f->modesets[0].crtc = f->crtc;
	f->modesets[0].connectors = f->connectors;
	mutex_init(&f->client.modeset_mutex);
	init_completion(&f->checked);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, finish_fixture, f), 0);
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
	return f;
}

static int release_reader(void *data)
{
	struct client_fixture *f = data;
	struct drm_modeset_acquire_ctx ctx;

	wait_for_completion(&f->checked);
	drm_modeset_acquire_init(&ctx, 0);
	f->worker_error = drm_modeset_lock(&f->crtc->mutex, &ctx);
	if (f->abandon)
		drm_prepare_read_abandon(f->read);
	else
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

static void start_reader(struct kunit *test, struct client_fixture *f)
{
	struct task_struct *worker = kthread_run(release_reader, f, "drm-client-reader");

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, worker);
	f->worker = worker;
}

static void join_reader(struct client_fixture *f)
{
	kthread_stop(f->worker);
	f->worker = NULL;
}

static void client_commit_waits_for_reader(struct kunit *test)
{
	struct client_fixture *f = new_client(test);

	start_reader(test, f);
	KUNIT_EXPECT_EQ(test, drm_client_modeset_commit(&f->client), 0);
	join_reader(f);
	KUNIT_EXPECT_EQ(test, f->worker_error, 0);
	KUNIT_EXPECT_EQ(test, f->checks, 2);
	KUNIT_EXPECT_EQ(test, f->installs, 1);
}

static void client_check_leaves_reader_pending(struct kunit *test)
{
	struct client_fixture *f = new_client(test);
	struct drm_crtc_state *before = f->crtc->state;
	struct drm_prepare_read_claim *another;

	KUNIT_EXPECT_EQ(test, drm_client_modeset_check(&f->client), 0);
	KUNIT_EXPECT_EQ(test, f->checks, 1);
	KUNIT_EXPECT_EQ(test, f->installs, 0);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state, before);
	another = drm_prepare_source_claim(f->source);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, another);
	drm_prepare_read_release(another, NULL);
}

static void client_power_off_waits_for_reader(struct kunit *test)
{
	struct client_fixture *f = new_client(test);

	start_reader(test, f);
	KUNIT_EXPECT_EQ(test, drm_client_modeset_dpms(&f->client, DRM_MODE_DPMS_OFF), 0);
	join_reader(f);
	KUNIT_EXPECT_EQ(test, f->worker_error, 0);
	KUNIT_EXPECT_EQ(test, f->checks, 2);
	KUNIT_EXPECT_EQ(test, f->installs, 1);
	KUNIT_EXPECT_FALSE(test, f->crtc->state->active);
}

static void client_commit_rejects_failed_preparation(struct kunit *test)
{
	struct client_fixture *f = new_client(test);
	struct drm_crtc_state *before = f->crtc->state;

	f->abandon = true;
	start_reader(test, f);
	KUNIT_EXPECT_EQ(test, drm_client_modeset_commit(&f->client), -EIO);
	join_reader(f);
	KUNIT_EXPECT_EQ(test, f->worker_error, 0);
	KUNIT_EXPECT_EQ(test, f->installs, 0);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state, before);
}

static struct kunit_case cases[] = {
	KUNIT_CASE(client_commit_waits_for_reader),
	KUNIT_CASE(client_check_leaves_reader_pending),
	KUNIT_CASE(client_power_off_waits_for_reader),
	KUNIT_CASE(client_commit_rejects_failed_preparation),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_prepare_client",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_LICENSE("GPL");
