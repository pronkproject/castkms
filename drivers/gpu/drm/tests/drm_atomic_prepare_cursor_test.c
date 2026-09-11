// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/completion.h>
#include <linux/kthread.h>
#include <drm/drm_atomic.h>
#include <drm/drm_atomic_helper.h>
#include <drm/drm_atomic_prepare.h>
#include <drm/drm_atomic_prepare_display.h>
#include <drm/drm_atomic_prepare_owner.h>
#include <drm/drm_atomic_uapi.h>
#include <drm/drm_connector.h>
#include <drm/drm_fourcc.h>
#include <drm/drm_framebuffer.h>
#include <drm/drm_kunit_helpers.h>
#include <kunit/test.h>

struct cursor_fixture {
	struct drm_device *dev;
	struct drm_crtc *crtc;
	struct drm_plane *cursor;
	struct drm_connector connector;
	struct drm_framebuffer *fb, *replacement;
	struct drm_prepare_owner *owner;
	struct drm_prepare_read_claim *read;
	struct task_struct *worker;
	struct completion checked;
	unsigned int checks, installs;
	int worker_error;
	bool change_image, change_position, revoke, reject;
};

static int check_update(struct drm_device *dev, struct drm_atomic_commit *state)
{
	struct cursor_fixture *f = dev->dev_private;
	int ret = drm_atomic_helper_check(dev, state);

	f->checks++;
	complete_all(&f->checked);
	return f->reject ? -EINVAL : ret;
}

static int install_update(struct drm_device *dev, struct drm_atomic_commit *state, bool nonblock)
{
	struct cursor_fixture *f = dev->dev_private;
	int ret = drm_atomic_helper_swap_state(state, false);

	if (!ret)
		f->installs++;
	return ret;
}

static const struct drm_mode_config_funcs config_funcs = {
	.atomic_check = check_update,
	.atomic_commit = install_update,
};

static const struct drm_plane_funcs plane_funcs = {
	.reset = drm_atomic_helper_plane_reset,
	.atomic_duplicate_state = drm_atomic_helper_plane_duplicate_state,
	.atomic_destroy_state = drm_atomic_helper_plane_destroy_state,
};

static const struct drm_plane_helper_funcs plane_helper_funcs = {};

static const struct drm_connector_funcs connector_funcs = {
	.reset = drm_atomic_helper_connector_reset,
	.atomic_duplicate_state = drm_atomic_helper_connector_duplicate_state,
	.atomic_destroy_state = drm_atomic_helper_connector_destroy_state,
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
	fb->format = drm_format_info(DRM_FORMAT_ARGB8888);
	ret = drm_framebuffer_init(dev, fb, &fb_funcs);
	if (ret)
		kfree(fb);
	KUNIT_ASSERT_EQ(test, ret, 0);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_fb, fb), 0);
	return fb;
}

static void finish_fixture(void *data)
{
	struct cursor_fixture *f = data;

	complete_all(&f->checked);
	if (f->worker)
		kthread_stop(f->worker);
	if (f->read)
		drm_prepare_read_abandon(f->read);
	drm_prepare_owner_put(f->owner);
}

static struct cursor_fixture *new_fixture(struct kunit *test)
{
	static const u32 formats[] = { DRM_FORMAT_ARGB8888 };
	const struct drm_display_mode mode = {
		DRM_MODE("64x64", 0, 1000, 64, 65, 66, 67, 0, 64, 65, 66, 67, 0, 0)
	};
	struct cursor_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct device *parent = drm_kunit_helper_alloc_device(test);
	struct drm_plane *primary;
	int ret;

	KUNIT_ASSERT_NOT_NULL(test, f);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	f->dev = __drm_kunit_helper_alloc_drm_device(test, parent, sizeof(*f->dev), 0,
						  DRIVER_MODESET | DRIVER_ATOMIC |
						  DRIVER_CURSOR_HOTSPOT);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->dev);
	f->dev->dev_private = f;
	f->dev->mode_config.funcs = &config_funcs;
	KUNIT_ASSERT_EQ(test, drm_atomic_prepare_display_init(f->dev, 8), 0);
	primary = drm_kunit_helper_create_primary_plane(test, f->dev, NULL, NULL,
						      formats, ARRAY_SIZE(formats), NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, primary);
	f->cursor = __drmm_universal_plane_alloc(f->dev, sizeof(*f->cursor), 0, 0,
						 &plane_funcs, formats, ARRAY_SIZE(formats),
						 NULL, DRM_PLANE_TYPE_CURSOR, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->cursor);
	drm_plane_helper_add(f->cursor, &plane_helper_funcs);
	f->crtc = drm_kunit_helper_create_crtc(test, f->dev, primary, f->cursor,
					      NULL, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->crtc);
	KUNIT_ASSERT_EQ(test, drmm_connector_init(f->dev, &f->connector, &connector_funcs,
						DRM_MODE_CONNECTOR_VIRTUAL, NULL), 0);
	drm_mode_config_reset(f->dev);
	f->fb = new_fb(test, f->dev);
	f->replacement = new_fb(test, f->dev);
	f->owner = drm_prepare_owner_create(8);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->owner);
	init_completion(&f->checked);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, finish_fixture, f), 0);
	ret = drm_modeset_lock(&f->crtc->mutex, NULL);
	KUNIT_ASSERT_EQ(test, ret, 0);
	ret = drm_atomic_set_mode_for_crtc(f->crtc->state, &mode);
	f->crtc->state->active = true;
	f->crtc->state->plane_mask = drm_plane_mask(f->cursor);
	f->crtc->state->connector_mask = drm_connector_mask(&f->connector);
	drm_connector_get(&f->connector);
	f->connector.state->crtc = f->crtc;
	f->cursor->state->crtc = f->crtc;
	drm_framebuffer_assign(&f->cursor->state->fb, f->fb);
	f->cursor->state->crtc_w = f->cursor->state->crtc_h = 64;
	f->cursor->state->src_w = f->cursor->state->src_h = 64 << 16;
	f->crtc->cursor_x = 3;
	f->crtc->cursor_y = 5;
	drm_modeset_unlock(&f->crtc->mutex);
	KUNIT_ASSERT_EQ(test, ret, 0);
	return f;
}

static int finish_reader(void *data)
{
	struct cursor_fixture *f = data;
	struct drm_modeset_acquire_ctx ctx;
	int ret;

	wait_for_completion(&f->checked);
	drm_modeset_acquire_init(&ctx, 0);
	for (;;) {
		ret = drm_modeset_lock_all_ctx(f->dev, &ctx);
		if (ret != -EDEADLK)
			break;
		ret = drm_modeset_backoff(&ctx);
		if (ret)
			break;
	}
	if (!ret) {
		if (f->change_image)
			drm_framebuffer_assign(&f->cursor->state->fb, f->replacement);
		if (f->change_position) {
			f->crtc->cursor_x = 31;
			f->crtc->cursor_y = 37;
		}
	}
	f->worker_error = ret;
	drm_modeset_drop_locks(&ctx);
	drm_modeset_acquire_fini(&ctx);
	if (f->revoke)
		drm_prepare_owner_revoke(f->owner);
	else {
		drm_prepare_read_release(f->read, NULL);
		f->read = NULL;
	}
	set_current_state(TASK_INTERRUPTIBLE);
	while (!kthread_should_stop()) {
		schedule();
		set_current_state(TASK_INTERRUPTIBLE);
	}
	__set_current_state(TASK_RUNNING);
	return 0;
}

static void start_reader(struct kunit *test, struct cursor_fixture *f)
{
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
	f->worker = kthread_run(finish_reader, f, "drm-cursor-reader");
	if (IS_ERR(f->worker)) {
		ret = PTR_ERR(f->worker);
		f->worker = NULL;
	}
	KUNIT_ASSERT_EQ(test, ret, 0);
}

static void join_reader(struct kunit *test, struct cursor_fixture *f)
{
	kthread_stop(f->worker);
	f->worker = NULL;
	KUNIT_EXPECT_EQ(test, f->worker_error, 0);
}

static void move_uses_current_image_after_wait(struct kunit *test)
{
	struct cursor_fixture *f = new_fixture(test);
	struct drm_cursor_update update = {
		.crtc = f->crtc, .update_position = true, .x = -17, .y = 29,
	};
	int ret;

	f->change_image = true;
	start_reader(test, f);
	ret = drm_atomic_helper_cursor_request(&update, f->owner, NULL, NULL);
	join_reader(test, f);
	KUNIT_EXPECT_EQ(test, ret, 0);
	KUNIT_EXPECT_GE(test, f->checks, 2);
	KUNIT_EXPECT_EQ(test, f->installs, 1);
	KUNIT_EXPECT_PTR_EQ(test, f->cursor->state->fb, f->replacement);
	KUNIT_EXPECT_EQ(test, f->cursor->state->crtc_x, -17);
	KUNIT_EXPECT_EQ(test, f->crtc->cursor_x, -17);
	KUNIT_EXPECT_EQ(test, f->crtc->cursor_y, 29);
}

static void image_uses_current_position_after_wait(struct kunit *test)
{
	struct cursor_fixture *f = new_fixture(test);
	struct drm_cursor_update update = {
		.crtc = f->crtc, .update_image = true, .fb = f->replacement,
	};
	int ret;

	f->change_position = true;
	start_reader(test, f);
	ret = drm_atomic_helper_cursor_request(&update, f->owner, NULL, NULL);
	join_reader(test, f);
	KUNIT_EXPECT_EQ(test, ret, 0);
	KUNIT_EXPECT_GE(test, f->checks, 2);
	KUNIT_EXPECT_EQ(test, f->installs, 1);
	KUNIT_EXPECT_PTR_EQ(test, f->cursor->state->fb, f->replacement);
	KUNIT_EXPECT_EQ(test, f->cursor->state->crtc_x, 31);
	KUNIT_EXPECT_EQ(test, f->cursor->state->crtc_y, 37);
}

static void revoked_move_preserves_position(struct kunit *test)
{
	struct cursor_fixture *f = new_fixture(test);
	struct drm_cursor_update update = {
		.crtc = f->crtc, .update_position = true, .x = 41, .y = 43,
	};
	int ret;

	f->revoke = true;
	start_reader(test, f);
	ret = drm_atomic_helper_cursor_request(&update, f->owner, NULL, NULL);
	join_reader(test, f);
	KUNIT_EXPECT_EQ(test, ret, -ECANCELED);
	KUNIT_EXPECT_EQ(test, f->installs, 0);
	KUNIT_EXPECT_EQ(test, f->crtc->cursor_x, 3);
	KUNIT_EXPECT_EQ(test, f->crtc->cursor_y, 5);
}

static void hidden_cursor_remembers_position(struct kunit *test)
{
	struct cursor_fixture *f = new_fixture(test);
	struct drm_cursor_update update = {
		.crtc = f->crtc, .update_image = true,
		.update_position = true, .x = 41, .y = 43,
	};

	KUNIT_ASSERT_EQ(test, drm_atomic_helper_cursor_request(&update, f->owner, NULL, NULL), 0);
	KUNIT_EXPECT_PTR_EQ(test, f->cursor->state->fb, NULL);
	KUNIT_EXPECT_PTR_EQ(test, f->cursor->state->crtc, NULL);
	KUNIT_EXPECT_EQ(test, f->cursor->state->src_w, 0);
	KUNIT_EXPECT_EQ(test, f->crtc->cursor_x, 41);
	KUNIT_EXPECT_EQ(test, f->crtc->cursor_y, 43);
}

static void rejected_image_preserves_hotspot(struct kunit *test)
{
	struct cursor_fixture *f = new_fixture(test);
	struct drm_cursor_update update = {
		.crtc = f->crtc, .update_image = true, .fb = f->replacement,
		.hot_x = 7, .hot_y = 11,
	};
	int ret;

	KUNIT_ASSERT_NOT_NULL(test, f->cursor->hotspot_x_property);
	KUNIT_ASSERT_NOT_NULL(test, f->cursor->hotspot_y_property);
	f->reject = true;
	ret = drm_atomic_helper_cursor_request(&update, f->owner, NULL, NULL);
	KUNIT_EXPECT_EQ(test, ret, -EINVAL);
	KUNIT_EXPECT_EQ(test, f->installs, 0);
	KUNIT_EXPECT_PTR_EQ(test, f->cursor->state->fb, f->fb);
	KUNIT_EXPECT_EQ(test, f->cursor->state->hotspot_x, 0);
	KUNIT_EXPECT_EQ(test, f->cursor->state->hotspot_y, 0);
}

static int accept_async_update(struct drm_plane *plane, struct drm_atomic_commit *state,
			       bool flip_async)
{
	return 0;
}

static void install_async_update(struct drm_plane *plane, struct drm_atomic_commit *state)
{
}

static void cursor_uses_normal_acceptance_on_async_capable_plane(struct kunit *test)
{
	static const struct drm_plane_helper_funcs async_helpers = {
		.atomic_async_check = accept_async_update,
		.atomic_async_update = install_async_update,
	};
	struct cursor_fixture *f = new_fixture(test);
	struct drm_cursor_update update = {
		.crtc = f->crtc, .update_position = true, .x = 41, .y = 43,
	};

	drm_plane_helper_add(f->cursor, &async_helpers);
	KUNIT_ASSERT_EQ(test, drm_atomic_helper_cursor_request(&update, f->owner, NULL, NULL), 0);
	KUNIT_EXPECT_EQ(test, f->installs, 1);
	KUNIT_EXPECT_EQ(test, f->crtc->cursor_x, 41);
	KUNIT_EXPECT_EQ(test, f->crtc->cursor_y, 43);
}

static struct kunit_case cases[] = {
	KUNIT_CASE(move_uses_current_image_after_wait),
	KUNIT_CASE(image_uses_current_position_after_wait),
	KUNIT_CASE(revoked_move_preserves_position),
	KUNIT_CASE(hidden_cursor_remembers_position),
	KUNIT_CASE(rejected_image_preserves_hotspot),
	KUNIT_CASE(cursor_uses_normal_acceptance_on_async_capable_plane),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_prepare_cursor",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_LICENSE("GPL");
