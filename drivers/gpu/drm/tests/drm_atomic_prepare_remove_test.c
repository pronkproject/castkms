// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <drm/drm_atomic.h>
#include <drm/drm_atomic_helper.h>
#include <drm/drm_atomic_prepare.h>
#include <drm/drm_atomic_prepare_display.h>
#include <drm/drm_atomic_uapi.h>
#include <drm/drm_fourcc.h>
#include <drm/drm_framebuffer.h>
#include <drm/drm_kunit_helpers.h>
#include <kunit/test.h>
#include <linux/completion.h>
#include <linux/kthread.h>

struct remove_fixture {
	struct drm_device *dev;
	struct drm_crtc *crtc;
	struct drm_plane *plane;
	struct drm_framebuffer *fb;
	struct drm_prepare_source *source;
	struct drm_prepare_read_claim *read;
	struct task_struct *worker;
	struct completion checked;
	unsigned int checks;
	unsigned int installs;
	int worker_error;
};

static int check_remove(struct drm_device *dev, struct drm_atomic_commit *state)
{
	struct remove_fixture *f = dev->dev_private;

	f->checks++;
	complete_all(&f->checked);
	return 0;
}

static int install_remove(struct drm_device *dev, struct drm_atomic_commit *state,
			  bool nonblock)
{
	struct remove_fixture *f = dev->dev_private;
	int ret = drm_atomic_helper_swap_state(state, false);

	if (!ret)
		f->installs++;
	return ret;
}

static const struct drm_mode_config_funcs remove_funcs = {
	.atomic_check = check_remove,
	.atomic_commit = install_remove,
};

static void destroy_fb(struct drm_framebuffer *fb)
{
	drm_framebuffer_cleanup(fb);
	kfree(fb);
}

static const struct drm_framebuffer_funcs fb_funcs = {
	.destroy = destroy_fb,
};

static void put_fb(void *data)
{
	drm_framebuffer_put(data);
}

static struct drm_framebuffer *new_fb(struct kunit *test, struct drm_device *dev)
{
	struct drm_framebuffer *fb = kzalloc(sizeof(*fb), GFP_KERNEL);
	int ret;

	KUNIT_ASSERT_NOT_NULL(test, fb);
	fb->dev = dev;
	fb->format = drm_format_info(DRM_FORMAT_XRGB8888);
	fb->width = fb->height = 64;
	fb->pitches[0] = 256;
	ret = drm_framebuffer_init(dev, fb, &fb_funcs);
	if (ret)
		kfree(fb);
	KUNIT_ASSERT_EQ(test, ret, 0);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_fb, fb), 0);
	return fb;
}

static void finish_fixture(void *data)
{
	struct remove_fixture *f = data;

	complete_all(&f->checked);
	if (f->worker)
		kthread_stop(f->worker);
	if (f->read)
		drm_prepare_read_abandon(f->read);
	if (f->source)
		drm_prepare_source_put(f->source);
}

static struct remove_fixture *new_remove(struct kunit *test)
{
	const struct drm_display_mode mode = {
		DRM_MODE("64x64", 0, 1000, 64, 65, 66, 67, 0, 64, 65, 66, 67, 0, 0)
	};
	struct remove_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct device *parent = drm_kunit_helper_alloc_device(test);
	int ret;

	KUNIT_ASSERT_NOT_NULL(test, f);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	f->dev = __drm_kunit_helper_alloc_drm_device(test, parent, sizeof(*f->dev), 0,
						  DRIVER_MODESET | DRIVER_ATOMIC);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->dev);
	f->dev->dev_private = f;
	f->dev->mode_config.funcs = &remove_funcs;
	KUNIT_ASSERT_EQ(test, drm_atomic_prepare_display_init(f->dev, 8), 0);
	f->plane = drm_kunit_helper_create_primary_plane(test, f->dev, NULL, NULL,
							 NULL, 0, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->plane);
	f->crtc = drm_kunit_helper_create_crtc(test, f->dev, f->plane, NULL, NULL, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->crtc);
	drm_mode_config_reset(f->dev);
	f->fb = new_fb(test, f->dev);
	init_completion(&f->checked);
	KUNIT_ASSERT_EQ(test, drm_modeset_lock(&f->crtc->mutex, NULL), 0);
	ret = drm_atomic_set_mode_for_crtc(f->crtc->state, &mode);
	f->crtc->state->active = true;
	f->crtc->state->plane_mask = drm_plane_mask(f->plane);
	f->plane->state->crtc = f->crtc;
	drm_framebuffer_assign(&f->plane->state->fb, f->fb);
	f->plane->state->crtc_w = f->plane->state->crtc_h = 64;
	f->plane->state->src_w = f->plane->state->src_h = 64 << 16;
	drm_modeset_unlock(&f->crtc->mutex);
	KUNIT_ASSERT_EQ(test, ret, 0);
	return f;
}

static int release_reader(void *data)
{
	struct remove_fixture *f = data;
	struct drm_modeset_acquire_ctx ctx;

	wait_for_completion(&f->checked);
	drm_modeset_acquire_init(&ctx, 0);
	for (;;) {
		f->worker_error = drm_modeset_lock_all_ctx(f->dev, &ctx);
		if (f->worker_error != -EDEADLK)
			break;
		f->worker_error = drm_modeset_backoff(&ctx);
		if (f->worker_error)
			break;
	}
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

static void start_reader(struct kunit *test, struct remove_fixture *f)
{
	struct drm_prepare_source *source;
	struct drm_prepare_read_claim *read = NULL;
	struct task_struct *worker;

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
	worker = kthread_run(release_reader, f, "drm-remove-reader");
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, worker);
	f->worker = worker;
}

static void removal_waits_for_reader_before_disabling_plane(struct kunit *test)
{
	struct remove_fixture *f = new_remove(test);

	start_reader(test, f);
	drm_framebuffer_get(f->fb);
	drm_framebuffer_remove(f->fb);
	kthread_stop(f->worker);
	f->worker = NULL;
	KUNIT_EXPECT_EQ(test, f->worker_error, 0);
	KUNIT_EXPECT_EQ(test, f->checks, 2);
	KUNIT_EXPECT_EQ(test, f->installs, 1);
	KUNIT_EXPECT_PTR_EQ(test, f->plane->state->fb, NULL);
	KUNIT_EXPECT_TRUE(test, f->crtc->state->active);
}

static struct kunit_case cases[] = {
	KUNIT_CASE(removal_waits_for_reader_before_disabling_plane),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_prepare_remove",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_LICENSE("GPL");
