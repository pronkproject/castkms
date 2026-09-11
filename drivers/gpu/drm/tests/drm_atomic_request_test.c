// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <drm/drm_atomic_request.h>
#include <drm/drm_crtc.h>
#include <drm/drm_fourcc.h>
#include <drm/drm_framebuffer.h>
#include <drm/drm_kunit_helpers.h>
#include <drm/drm_plane.h>
#include <drm/drm_property.h>
#include <kunit/test.h>

struct request_fixture {
	struct drm_device *dev;
	struct drm_plane *plane;
};

static struct request_fixture *new_fixture(struct kunit *test, struct device *parent)
{
	struct request_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);

	KUNIT_ASSERT_NOT_NULL(test, f);
	if (!parent)
		parent = drm_kunit_helper_alloc_device(test);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	f->dev = __drm_kunit_helper_alloc_drm_device(test, parent, sizeof(*f->dev), 0,
						  DRIVER_MODESET | DRIVER_ATOMIC);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->dev);
	f->plane = drm_kunit_helper_create_primary_plane(test, f->dev, NULL, NULL,
							 NULL, 0, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->plane);
	return f;
}

static void destroy_request(void *data)
{
	drm_atomic_request_destroy(data);
}

static struct drm_atomic_request *new_request(struct kunit *test, struct drm_device *dev,
					      struct drm_atomic_request_entry *entries,
					      unsigned int count)
{
	struct drm_atomic_request *request = drm_atomic_request_create(dev, entries, count);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, request);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, destroy_request, request), 0);
	return request;
}

static void scalar_entries_are_copied_in_order(struct kunit *test)
{
	struct request_fixture *f = new_fixture(test, NULL);
	struct drm_atomic_request_entry entries[2] = {
		{ .object = &f->plane->base, .property = f->dev->mode_config.prop_crtc_w,
		  .type = DRM_ATOMIC_REQUEST_SCALAR, .scalar = 10 },
		{ .object = &f->plane->base, .property = f->dev->mode_config.prop_crtc_w,
		  .type = DRM_ATOMIC_REQUEST_SCALAR, .scalar = 20 },
	};
	struct drm_atomic_request *request = new_request(test, f->dev, entries, 2);

	memset(entries, 0, sizeof(entries));
	KUNIT_EXPECT_EQ(test, drm_atomic_request_count(request), 2);
	KUNIT_EXPECT_EQ(test, drm_atomic_request_entry(request, 0)->scalar, 10);
	KUNIT_EXPECT_EQ(test, drm_atomic_request_entry(request, 1)->scalar, 20);
	KUNIT_EXPECT_PTR_EQ(test, drm_atomic_request_entry(request, 2), NULL);
}

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

static void framebuffer_survives_creator_release(struct kunit *test)
{
	struct request_fixture *f = new_fixture(test, NULL);
	struct drm_framebuffer *fb = kzalloc_obj(*fb);
	struct drm_atomic_request_entry entry = {
		.object = &f->plane->base, .property = f->dev->mode_config.prop_fb_id,
		.type = DRM_ATOMIC_REQUEST_FRAMEBUFFER, .framebuffer = fb,
	};
	struct drm_atomic_request *request;
	int ret;

	KUNIT_ASSERT_NOT_NULL(test, fb);
	fb->dev = f->dev;
	fb->format = drm_format_info(DRM_FORMAT_XRGB8888);
	fb->width = 64;
	ret = drm_framebuffer_init(f->dev, fb, &fb_funcs);
	if (ret)
		kfree(fb);
	KUNIT_ASSERT_EQ(test, ret, 0);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_fb, fb), 0);
	request = new_request(test, f->dev, &entry, 1);
	kunit_release_action(test, put_fb, fb);
	KUNIT_EXPECT_PTR_EQ(test, drm_atomic_request_entry(request, 0)->framebuffer, fb);
	KUNIT_EXPECT_EQ(test, fb->width, 64);
	KUNIT_EXPECT_EQ(test, kref_read(&fb->base.refcount), 1);
}

static void put_blob(void *data)
{
	drm_property_blob_put(data);
}

static void blob_survives_creator_release(struct kunit *test)
{
	struct request_fixture *f = new_fixture(test, NULL);
	u32 contents = 123;
	struct drm_property *prop = drm_property_create(f->dev,
				DRM_MODE_PROP_ATOMIC | DRM_MODE_PROP_BLOB, "payload", 0);
	struct drm_property_blob *blob;
	struct drm_atomic_request_entry entry = {
		.object = &f->plane->base, .property = prop,
		.type = DRM_ATOMIC_REQUEST_BLOB,
	};
	struct drm_atomic_request *request;

	KUNIT_ASSERT_NOT_NULL(test, prop);
	blob = drm_property_create_blob(f->dev, sizeof(contents), &contents);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, blob);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_blob, blob), 0);
	entry.blob = blob;
	drm_object_attach_property(&f->plane->base, prop, 0);
	request = new_request(test, f->dev, &entry, 1);
	kunit_release_action(test, put_blob, blob);
	KUNIT_EXPECT_PTR_EQ(test, drm_atomic_request_entry(request, 0)->blob, blob);
	KUNIT_EXPECT_EQ(test, *(u32 *)blob->data, contents);
	KUNIT_EXPECT_EQ(test, kref_read(&blob->base.refcount), 1);
}

static void wrong_value_kind_is_rejected(struct kunit *test)
{
	struct request_fixture *f = new_fixture(test, NULL);
	struct drm_atomic_request_entry entry = {
		.object = &f->plane->base, .property = f->dev->mode_config.prop_fb_id,
		.type = DRM_ATOMIC_REQUEST_SCALAR, .scalar = 0,
	};
	struct drm_atomic_request *request = drm_atomic_request_create(f->dev, &entry, 1);

	KUNIT_EXPECT_TRUE(test, IS_ERR(request));
	if (IS_ERR(request))
		KUNIT_EXPECT_EQ(test, PTR_ERR(request), -EINVAL);
	else
		drm_atomic_request_destroy(request);
}

static void foreign_target_is_rejected(struct kunit *test)
{
	struct request_fixture *a = new_fixture(test, NULL);
	struct request_fixture *b = new_fixture(test, a->dev->dev);
	struct drm_atomic_request_entry entry = {
		.object = &b->plane->base, .property = a->dev->mode_config.prop_crtc_w,
		.type = DRM_ATOMIC_REQUEST_SCALAR, .scalar = 10,
	};
	struct drm_atomic_request *request = drm_atomic_request_create(a->dev, &entry, 1);

	KUNIT_EXPECT_TRUE(test, IS_ERR(request));
	if (IS_ERR(request))
		KUNIT_EXPECT_EQ(test, PTR_ERR(request), -EINVAL);
	else
		drm_atomic_request_destroy(request);
}

static void resolved_controller_identity_is_copied(struct kunit *test)
{
	struct request_fixture *f = new_fixture(test, NULL);
	struct drm_crtc *crtc = drm_kunit_helper_create_crtc(test, f->dev, f->plane,
								    NULL, NULL, NULL);
	struct drm_atomic_request_entry entry = {
		.object = &f->plane->base, .property = f->dev->mode_config.prop_crtc_id,
		.type = DRM_ATOMIC_REQUEST_OBJECT,
	};
	struct drm_atomic_request *request;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, crtc);
	entry.reference = &crtc->base;
	request = new_request(test, f->dev, &entry, 1);
	entry.reference = NULL;
	KUNIT_EXPECT_PTR_EQ(test, drm_atomic_request_entry(request, 0)->reference,
			    &crtc->base);
}

static void output_pointer_is_not_a_retained_value(struct kunit *test)
{
	struct request_fixture *f = new_fixture(test, NULL);
	struct drm_crtc *crtc = drm_kunit_helper_create_crtc(test, f->dev, f->plane,
								    NULL, NULL, NULL);
	struct drm_atomic_request_entry entry = {
		.property = f->dev->mode_config.prop_out_fence_ptr,
		.type = DRM_ATOMIC_REQUEST_SCALAR, .scalar = 0,
	};
	struct drm_atomic_request *request;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, crtc);
	entry.object = &crtc->base;
	request = drm_atomic_request_create(f->dev, &entry, 1);
	KUNIT_EXPECT_TRUE(test, IS_ERR(request));
	if (IS_ERR(request))
		KUNIT_EXPECT_EQ(test, PTR_ERR(request), -EOPNOTSUPP);
	else
		drm_atomic_request_destroy(request);
}

static struct kunit_case cases[] = {
	KUNIT_CASE(scalar_entries_are_copied_in_order),
	KUNIT_CASE(framebuffer_survives_creator_release),
	KUNIT_CASE(blob_survives_creator_release),
	KUNIT_CASE(wrong_value_kind_is_rejected),
	KUNIT_CASE(foreign_target_is_rejected),
	KUNIT_CASE(resolved_controller_identity_is_copied),
	KUNIT_CASE(output_pointer_is_not_a_retained_value),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_request",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_LICENSE("GPL");
