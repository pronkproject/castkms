// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <drm/drm_atomic.h>
#include <drm/drm_atomic_request.h>
#include <drm/drm_fourcc.h>
#include <drm/drm_framebuffer.h>
#include <drm/drm_kunit_helpers.h>
#include <drm/drm_property.h>
#include <kunit/test.h>

struct apply_fixture {
	struct drm_device *dev;
	struct drm_plane *plane;
	struct drm_crtc *crtc;
	struct drm_atomic_commit *state;
	unsigned int validations;
	int validation_error;
};

static void finish_state(void *data)
{
	struct apply_fixture *f = data;

	drm_atomic_commit_put(f->state);
}

static struct apply_fixture *new_fixture(struct kunit *test)
{
	struct apply_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct device *parent = drm_kunit_helper_alloc_device(test);

	KUNIT_ASSERT_NOT_NULL(test, f);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	f->dev = __drm_kunit_helper_alloc_drm_device(test, parent, sizeof(*f->dev), 0,
						  DRIVER_MODESET | DRIVER_ATOMIC);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->dev);
	f->plane = drm_kunit_helper_create_primary_plane(test, f->dev, NULL, NULL,
							 NULL, 0, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->plane);
	f->crtc = drm_kunit_helper_create_crtc(test, f->dev, f->plane, NULL, NULL, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->crtc);
	drm_plane_enable_fb_damage_clips(f->plane);
	drm_mode_config_reset(f->dev);
	f->state = drm_atomic_commit_alloc(f->dev);
	KUNIT_ASSERT_NOT_NULL(test, f->state);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, finish_state, f), 0);
	return f;
}

/* KUnit assertions and cleanup run only after the acquire context is finished. */
static int apply_request(const struct drm_atomic_request *request, struct drm_atomic_commit *state,
			 int (*validate)(struct drm_atomic_commit *, const struct drm_atomic_request *,
					 void *), void *data)
{
	struct drm_modeset_acquire_ctx ctx;
	int ret;

	drm_modeset_acquire_init(&ctx, 0);
	state->acquire_ctx = &ctx;
	for (;;) {
		ret = drm_atomic_request_apply(request, state, validate, data);
		if (ret != -EDEADLK)
			break;
		drm_atomic_commit_clear(state);
		ret = drm_modeset_backoff(&ctx);
		if (ret)
			break;
	}
	drm_modeset_drop_locks(&ctx);
	state->acquire_ctx = NULL;
	drm_modeset_acquire_fini(&ctx);
	return ret;
}

static int validate_request(struct drm_atomic_commit *state,
			    const struct drm_atomic_request *request, void *data)
{
	struct apply_fixture *f = data;

	f->validations++;
	return f->validation_error;
}

static void destroy_request(void *data)
{
	drm_atomic_request_destroy(data);
}

static struct drm_atomic_request *new_request(struct kunit *test, struct apply_fixture *f,
					      struct drm_atomic_request_entry *entries,
					      unsigned int count)
{
	struct drm_atomic_request *request = drm_atomic_request_create(f->dev, entries, count);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, request);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, destroy_request, request), 0);
	return request;
}

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
	fb->format = drm_format_info(DRM_FORMAT_XRGB8888);
	fb->width = 64;
	ret = drm_framebuffer_init(dev, fb, &fb_funcs);
	if (ret)
		kfree(fb);
	KUNIT_ASSERT_EQ(test, ret, 0);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_fb, fb), 0);
	return fb;
}

static void framebuffer_is_reapplied_after_clear(struct kunit *test)
{
	struct apply_fixture *f = new_fixture(test);
	struct drm_framebuffer *fb = new_fb(test, f->dev);
	struct drm_atomic_request_entry entry = {
		.object = &f->plane->base, .property = f->dev->mode_config.prop_fb_id,
		.type = DRM_ATOMIC_REQUEST_FRAMEBUFFER, .framebuffer = fb,
	};
	struct drm_atomic_request *request = new_request(test, f, &entry, 1);
	unsigned int i;

	kunit_release_action(test, put_fb, fb);
	for (i = 0; i < 2; i++) {
		KUNIT_ASSERT_EQ(test, apply_request(request, f->state, validate_request, f), 0);
		KUNIT_EXPECT_PTR_EQ(test, drm_atomic_get_new_plane_state(f->state, f->plane)->fb, fb);
		KUNIT_EXPECT_TRUE(test, drm_atomic_get_new_plane_state(f->state, f->plane)->fb_set);
		KUNIT_EXPECT_EQ(test, kref_read(&fb->base.refcount), 2);
		drm_atomic_commit_clear(f->state);
		KUNIT_EXPECT_EQ(test, kref_read(&fb->base.refcount), 1);
	}
	KUNIT_EXPECT_EQ(test, f->validations, 2);
}

static void authority_is_rechecked_on_rebuild(struct kunit *test)
{
	struct apply_fixture *f = new_fixture(test);
	struct drm_atomic_request_entry entry = {
		.object = &f->plane->base, .property = f->dev->mode_config.prop_fb_id,
		.type = DRM_ATOMIC_REQUEST_FRAMEBUFFER,
	};
	struct drm_atomic_request *request = new_request(test, f, &entry, 1);

	KUNIT_ASSERT_EQ(test, apply_request(request, f->state, validate_request, f), 0);
	drm_atomic_commit_clear(f->state);
	f->validation_error = -EACCES;
	KUNIT_EXPECT_EQ(test, apply_request(request, f->state, validate_request, f),
			-EACCES);
	KUNIT_EXPECT_EQ(test, f->validations, 2);
	KUNIT_EXPECT_PTR_EQ(test, drm_atomic_get_new_plane_state(f->state, f->plane), NULL);
}

static void unsupported_property_prevents_application(struct kunit *test)
{
	struct apply_fixture *f = new_fixture(test);
	struct drm_property *prop = drm_property_create_range(f->dev, DRM_MODE_PROP_ATOMIC,
							    "private-scalar", 0, 1);
	struct drm_atomic_request_entry entries[2] = {
		{ .object = &f->plane->base, .property = f->dev->mode_config.prop_fb_id,
		  .type = DRM_ATOMIC_REQUEST_FRAMEBUFFER },
		{ .object = &f->plane->base, .property = prop,
		  .type = DRM_ATOMIC_REQUEST_SCALAR, .scalar = 1 },
	};
	struct drm_atomic_request *request;

	KUNIT_ASSERT_NOT_NULL(test, prop);
	drm_object_attach_property(&f->plane->base, prop, 0);
	request = new_request(test, f, entries, 2);
	KUNIT_EXPECT_EQ(test, apply_request(request, f->state, validate_request, f),
			-EOPNOTSUPP);
	KUNIT_EXPECT_EQ(test, f->validations, 0);
	KUNIT_EXPECT_PTR_EQ(test, drm_atomic_get_new_plane_state(f->state, f->plane), NULL);
}

static void authority_callback_is_required(struct kunit *test)
{
	struct apply_fixture *f = new_fixture(test);
	struct drm_atomic_request_entry entry = {
		.object = &f->plane->base, .property = f->dev->mode_config.prop_fb_id,
		.type = DRM_ATOMIC_REQUEST_FRAMEBUFFER,
	};
	struct drm_atomic_request *request = new_request(test, f, &entry, 1);

	KUNIT_EXPECT_EQ(test, apply_request(request, f->state, NULL, f), -EINVAL);
	KUNIT_EXPECT_PTR_EQ(test, drm_atomic_get_new_plane_state(f->state, f->plane), NULL);
}

static struct kunit_case apply_tests[] = {
	KUNIT_CASE(framebuffer_is_reapplied_after_clear),
	KUNIT_CASE(authority_is_rechecked_on_rebuild),
	KUNIT_CASE(unsupported_property_prevents_application),
	KUNIT_CASE(authority_callback_is_required),
	{ }
};

static struct kunit_suite apply_suite = {
	.name = "drm_atomic_request_apply",
	.test_cases = apply_tests,
};

kunit_test_suite(apply_suite);
MODULE_LICENSE("GPL");
