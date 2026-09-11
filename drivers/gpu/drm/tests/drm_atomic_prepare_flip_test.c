// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/completion.h>
#include <linux/file.h>
#include <linux/kthread.h>
#include <drm/drm_atomic.h>
#include <drm/drm_atomic_flip.h>
#include <drm/drm_atomic_helper.h>
#include <drm/drm_atomic_prepare.h>
#include <drm/drm_atomic_prepare_display.h>
#include <drm/drm_atomic_uapi.h>
#include <drm/drm_auth.h>
#include <drm/drm_file.h>
#include <drm/drm_fourcc.h>
#include <drm/drm_framebuffer.h>
#include <drm/drm_ioctl.h>
#include <drm/drm_kunit_helpers.h>
#include <kunit/test.h>

#include "../drm_atomic_user_commit.h"

struct flip_fixture {
	struct drm_device *dev;
	struct drm_crtc *crtc;
	struct drm_framebuffer *old, *next, *during;
	struct file *file;
	struct drm_prepare_read_claim *read;
	struct task_struct *worker;
	struct completion checked;
	unsigned int checks, installs;
	int worker_error;
	bool nonblock;
};

static int check_update(struct drm_device *dev, struct drm_atomic_commit *state)
{
	struct flip_fixture *f = dev->dev_private;

	f->checks++;
	complete_all(&f->checked);
	return 0;
}

static int install_update(struct drm_device *dev, struct drm_atomic_commit *state, bool nonblock)
{
	struct flip_fixture *f = dev->dev_private;
	int ret = drm_atomic_helper_swap_state(state, false);

	if (!ret) {
		f->installs++;
		f->nonblock = nonblock;
	}
	return ret;
}

static const struct drm_mode_config_funcs config_funcs = {
	.atomic_check = check_update,
	.atomic_commit = install_update,
};

static const struct drm_crtc_funcs crtc_funcs = {
	.reset = drm_atomic_helper_crtc_reset,
	.atomic_duplicate_state = drm_atomic_helper_crtc_duplicate_state,
	.atomic_destroy_state = drm_atomic_helper_crtc_destroy_state,
	.build_page_flip = drm_atomic_set_legacy_flip,
};

static const struct file_operations file_ops = {
	.owner = THIS_MODULE,
	.release = drm_release_noglobal,
};

static const struct drm_driver driver = {
	.driver_features = DRIVER_MODESET | DRIVER_ATOMIC,
	.fops = &file_ops,
};

static void destroy_fb(struct drm_framebuffer *fb)
{
	drm_framebuffer_cleanup(fb);
	kfree(fb);
}

static const struct drm_framebuffer_funcs fb_funcs = { .destroy = destroy_fb };

static void put_fb(void *data)
{
	drm_framebuffer_put(data);
}

static struct drm_framebuffer *new_fb(struct kunit *test, struct drm_device *dev)
{
	struct drm_framebuffer *fb = kzalloc_obj(*fb);
	int ret;

	KUNIT_ASSERT_NOT_NULL(test, fb);
	fb->dev = dev;
	fb->width = fb->height = 64;
	fb->pitches[0] = 256;
	fb->format = drm_format_info(DRM_FORMAT_XRGB8888);
	ret = drm_framebuffer_init(dev, fb, &fb_funcs);
	if (ret)
		kfree(fb);
	KUNIT_ASSERT_EQ(test, ret, 0);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_fb, fb), 0);
	return fb;
}

static void close_file(void *data)
{
	__fput_sync(data);
}

static void finish_fixture(void *data)
{
	struct flip_fixture *f = data;

	complete_all(&f->checked);
	if (f->worker)
		kthread_stop(f->worker);
	if (f->read)
		drm_prepare_read_abandon(f->read);
}

static int lock_all(struct drm_device *dev, struct drm_modeset_acquire_ctx *ctx)
{
	int ret;

	for (;;) {
		ret = drm_modeset_lock_all_ctx(dev, ctx);
		if (ret != -EDEADLK)
			return ret;
		ret = drm_modeset_backoff(ctx);
		if (ret)
			return ret;
	}
}

static struct flip_fixture *new_fixture(struct kunit *test)
{
	const struct drm_display_mode mode = {
		DRM_MODE("64x64", 0, 1000, 64, 65, 66, 67, 0, 64, 65, 66, 67, 0, 0)
	};
	struct flip_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct device *parent = drm_kunit_helper_alloc_device(test);
	struct drm_plane *plane;
	struct drm_file *file;
	struct drm_master *master;
	struct drm_modeset_acquire_ctx ctx;
	int ret;

	KUNIT_ASSERT_NOT_NULL(test, f);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	f->dev = __drm_kunit_helper_alloc_drm_device_with_driver(test, parent,
							   sizeof(*f->dev), 0, &driver);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->dev);
	f->dev->dev_private = f;
	f->dev->mode_config.funcs = &config_funcs;
	KUNIT_ASSERT_EQ(test, drm_atomic_prepare_display_init(f->dev, 8), 0);
	plane = drm_kunit_helper_create_primary_plane(test, f->dev, NULL, NULL, NULL, 0, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, plane);
	f->crtc = drm_kunit_helper_create_crtc(test, f->dev, plane, NULL, &crtc_funcs, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->crtc);
	drm_mode_config_reset(f->dev);
	f->old = new_fb(test, f->dev);
	f->next = new_fb(test, f->dev);
	f->file = mock_drm_getfile(f->dev->primary, O_RDWR);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->file);
	atomic_inc(&f->dev->open_count);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, close_file, f->file), 0);
	file = f->file->private_data;
	master = kzalloc_obj(*master);
	KUNIT_ASSERT_NOT_NULL(test, master);
	kref_init(&master->refcount);
	master->dev = f->dev;
	idr_init_base(&master->magic_map, 1);
	idr_init(&master->leases);
	idr_init_base(&master->lessee_idr, 1);
	INIT_LIST_HEAD(&master->lessees);
	INIT_LIST_HEAD(&master->lessee_list);
	file->master = master;
	file->is_master = file->was_master = true;
	mutex_lock(&f->dev->master_mutex);
	f->dev->master = drm_master_get(master);
	mutex_unlock(&f->dev->master_mutex);
	init_completion(&f->checked);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, finish_fixture, f), 0);
	drm_modeset_acquire_init(&ctx, 0);
	ret = lock_all(f->dev, &ctx);
	if (!ret) {
		ret = drm_atomic_set_mode_for_crtc(f->crtc->state, &mode);
		f->crtc->state->active = true;
		f->crtc->state->plane_mask = drm_plane_mask(plane);
		plane->state->crtc = f->crtc;
		drm_framebuffer_assign(&plane->state->fb, f->old);
		plane->state->crtc_w = plane->state->crtc_h = 64;
		plane->state->src_w = plane->state->src_h = 64 << 16;
	}
	drm_modeset_drop_locks(&ctx);
	drm_modeset_acquire_fini(&ctx);
	KUNIT_ASSERT_EQ(test, ret, 0);
	return f;
}

static int finish_reader(void *data)
{
	struct flip_fixture *f = data;
	struct drm_modeset_acquire_ctx ctx;
	int ret;

	wait_for_completion(&f->checked);
	drm_modeset_acquire_init(&ctx, 0);
	ret = lock_all(f->dev, &ctx);
	if (!ret)
		f->during = f->crtc->primary->state->fb;
	drm_modeset_drop_locks(&ctx);
	drm_modeset_acquire_fini(&ctx);
	f->worker_error = ret;
	drm_prepare_read_release(f->read, NULL);
	f->read = NULL;
	set_current_state(TASK_INTERRUPTIBLE);
	while (!kthread_should_stop()) {
		schedule();
		set_current_state(TASK_INTERRUPTIBLE);
	}
	__set_current_state(TASK_RUNNING);
	return 0;
}

static int submit_with_reader(struct kunit *test, struct flip_fixture *f)
{
	struct drm_mode_crtc_page_flip_target input = { .fb_id = f->next->base.id };
	struct drm_prepare_source *source;
	struct drm_prepare_read_claim *read = NULL;
	int ret = drm_modeset_lock(&f->crtc->mutex, NULL);

	KUNIT_ASSERT_EQ(test, ret, 0);
	source = drm_atomic_prepare_crtc_source(f->crtc);
	if (!IS_ERR(source))
		read = drm_prepare_source_claim(source);
	drm_modeset_unlock(&f->crtc->mutex);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, source);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, read);
	f->read = read;
	f->worker = kthread_run(finish_reader, f, "drm-flip-reader");
	if (IS_ERR(f->worker)) {
		ret = PTR_ERR(f->worker);
		f->worker = NULL;
	}
	KUNIT_ASSERT_EQ(test, ret, 0);
	ret = drm_atomic_submit_user_flip(f->crtc, &input, f->file->private_data);
	complete_all(&f->checked);
	kthread_stop(f->worker);
	f->worker = NULL;
	KUNIT_EXPECT_EQ(test, f->worker_error, 0);
	return ret;
}

static void flip_waits_before_native_submission(struct kunit *test)
{
	struct flip_fixture *f = new_fixture(test);

	KUNIT_EXPECT_EQ(test, submit_with_reader(test, f), 0);
	KUNIT_EXPECT_GE(test, f->checks, 2);
	KUNIT_EXPECT_EQ(test, f->installs, 1);
	KUNIT_EXPECT_TRUE(test, f->nonblock);
	KUNIT_EXPECT_PTR_EQ(test, f->during, f->old);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->primary->state->fb, f->next);
}

static struct kunit_case cases[] = {
	KUNIT_CASE(flip_waits_before_native_submission),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_prepare_flip",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_IMPORT_NS("EXPORTED_FOR_KUNIT_TESTING");
MODULE_LICENSE("GPL");
