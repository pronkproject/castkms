// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <drm/drm_atomic.h>
#include <drm/drm_atomic_flip.h>
#include <drm/drm_atomic_uapi.h>
#include <drm/drm_fourcc.h>
#include <drm/drm_framebuffer.h>
#include <drm/drm_kunit_helpers.h>
#include <kunit/test.h>

struct flip_fixture {
	struct drm_device *dev;
	struct drm_crtc *crtc;
	struct drm_plane *plane;
	struct drm_framebuffer *old, *next;
	struct drm_atomic_commit *state;
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

static struct drm_framebuffer *new_fb(struct kunit *test, struct drm_device *dev,
					 uint32_t format, unsigned int width)
{
	struct drm_framebuffer *fb = kzalloc_obj(*fb);
	int ret;

	KUNIT_ASSERT_NOT_NULL(test, fb);
	fb->dev = dev;
	fb->width = width;
	fb->height = 64;
	fb->pitches[0] = width * 4;
	fb->format = drm_format_info(format);
	ret = drm_framebuffer_init(dev, fb, &fb_funcs);
	if (ret)
		kfree(fb);
	KUNIT_ASSERT_EQ(test, ret, 0);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_fb, fb), 0);
	return fb;
}

static void put_state(void *data)
{
	drm_atomic_commit_put(data);
}

static int run_update(struct flip_fixture *f, int (*operation)(struct flip_fixture *))
{
	struct drm_modeset_acquire_ctx ctx;
	int ret;

	drm_modeset_acquire_init(&ctx, 0);
	f->state->acquire_ctx = &ctx;
	for (;;) {
		ret = drm_modeset_lock_all_ctx(f->dev, &ctx);
		if (!ret)
			ret = operation(f);
		if (ret != -EDEADLK)
			break;
		drm_atomic_commit_clear(f->state);
		ret = drm_modeset_backoff(&ctx);
		if (ret)
			break;
	}
	drm_modeset_drop_locks(&ctx);
	f->state->acquire_ctx = NULL;
	drm_modeset_acquire_fini(&ctx);
	return ret;
}

static int initialize_display(struct flip_fixture *f)
{
	struct drm_plane_state *plane = f->plane->state;

	f->crtc->state->active = true;
	f->crtc->state->plane_mask = drm_plane_mask(f->plane);
	plane->crtc = f->crtc;
	drm_framebuffer_assign(&plane->fb, f->old);
	plane->crtc_x = 3;
	plane->crtc_y = 5;
	plane->crtc_w = plane->crtc_h = 32;
	plane->src_x = 8 << 16;
	plane->src_y = 4 << 16;
	plane->src_w = plane->src_h = 32 << 16;
	return 0;
}

static struct flip_fixture *new_fixture(struct kunit *test)
{
	struct flip_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct device *parent = drm_kunit_helper_alloc_device(test);

	KUNIT_ASSERT_NOT_NULL(test, f);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	f->dev = __drm_kunit_helper_alloc_drm_device(test, parent, sizeof(*f->dev), 0,
						  DRIVER_MODESET | DRIVER_ATOMIC);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->dev);
	f->plane = drm_kunit_helper_create_primary_plane(test, f->dev, NULL, NULL, NULL, 0, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->plane);
	f->crtc = drm_kunit_helper_create_crtc(test, f->dev, f->plane, NULL, NULL, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->crtc);
	drm_mode_config_reset(f->dev);
	f->old = new_fb(test, f->dev, DRM_FORMAT_XRGB8888, 64);
	f->next = new_fb(test, f->dev, DRM_FORMAT_XRGB8888, 64);
	f->state = drm_atomic_commit_alloc(f->dev);
	KUNIT_ASSERT_NOT_NULL(test, f->state);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_state, f->state), 0);
	KUNIT_ASSERT_EQ(test, run_update(f, initialize_display), 0);
	return f;
}

static int set_flip(struct flip_fixture *f)
{
	return drm_atomic_set_legacy_flip(f->state, f->crtc, f->next);
}

static void flip_preserves_geometry_and_accepted_image(struct kunit *test)
{
	struct flip_fixture *f = new_fixture(test);
	struct drm_plane_state *new;

	KUNIT_ASSERT_EQ(test, run_update(f, set_flip), 0);
	new = drm_atomic_get_new_plane_state(f->state, f->plane);
	KUNIT_EXPECT_PTR_EQ(test, new->fb, f->next);
	KUNIT_EXPECT_PTR_EQ(test, f->plane->state->fb, f->old);
	KUNIT_EXPECT_TRUE(test, new->fb_set);
	KUNIT_EXPECT_FALSE(test, f->state->allow_modeset);
	KUNIT_EXPECT_EQ(test, new->crtc_x, 3);
	KUNIT_EXPECT_EQ(test, new->crtc_y, 5);
	KUNIT_EXPECT_EQ(test, new->crtc_w, 32);
	KUNIT_EXPECT_EQ(test, new->crtc_h, 32);
	KUNIT_EXPECT_EQ(test, new->src_x, 8 << 16);
	KUNIT_EXPECT_EQ(test, new->src_y, 4 << 16);
	KUNIT_EXPECT_EQ(test, new->src_w, 32 << 16);
	KUNIT_EXPECT_EQ(test, new->src_h, 32 << 16);
}

static int flip_disabled(struct flip_fixture *f)
{
	f->crtc->state->active = false;
	return set_flip(f);
}

static void disabled_controller_rejects_flip(struct kunit *test)
{
	struct flip_fixture *f = new_fixture(test);

	KUNIT_EXPECT_EQ(test, run_update(f, flip_disabled), -EINVAL);
	KUNIT_EXPECT_PTR_EQ(test, f->plane->state->fb, f->old);
}

static int flip_without_image(struct flip_fixture *f)
{
	drm_framebuffer_assign(&f->plane->state->fb, NULL);
	return set_flip(f);
}

static void missing_current_image_rejects_flip(struct kunit *test)
{
	struct flip_fixture *f = new_fixture(test);

	KUNIT_EXPECT_EQ(test, run_update(f, flip_without_image), -EBUSY);
	KUNIT_EXPECT_NULL(test, drm_atomic_get_new_plane_state(f->state, f->plane)->fb);
}

static void different_format_rejects_flip(struct kunit *test)
{
	struct flip_fixture *f = new_fixture(test);

	f->next = new_fb(test, f->dev, DRM_FORMAT_ARGB8888, 64);
	KUNIT_EXPECT_EQ(test, run_update(f, set_flip), -EINVAL);
	KUNIT_EXPECT_PTR_EQ(test, drm_atomic_get_new_plane_state(f->state, f->plane)->fb, f->old);
}

static void smaller_image_rejects_source_rectangle(struct kunit *test)
{
	struct flip_fixture *f = new_fixture(test);

	f->next = new_fb(test, f->dev, DRM_FORMAT_XRGB8888, 32);
	KUNIT_EXPECT_EQ(test, run_update(f, set_flip), -ENOSPC);
	KUNIT_EXPECT_PTR_EQ(test, drm_atomic_get_new_plane_state(f->state, f->plane)->fb, f->old);
}

static struct kunit_case cases[] = {
	KUNIT_CASE(flip_preserves_geometry_and_accepted_image),
	KUNIT_CASE(disabled_controller_rejects_flip),
	KUNIT_CASE(missing_current_image_rejects_flip),
	KUNIT_CASE(different_format_rejects_flip),
	KUNIT_CASE(smaller_image_rejects_source_rectangle),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_flip",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_LICENSE("GPL");
