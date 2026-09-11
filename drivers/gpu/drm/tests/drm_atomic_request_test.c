// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <drm/drm_atomic_request.h>
#include <drm/drm_kunit_helpers.h>
#include <drm/drm_plane.h>
#include <drm/drm_property.h>
#include <kunit/test.h>

struct request_fixture {
	struct drm_device *dev;
	struct drm_plane *plane;
};

static struct request_fixture *new_fixture(struct kunit *test)
{
	struct request_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct device *parent = drm_kunit_helper_alloc_device(test);

	KUNIT_ASSERT_NOT_NULL(test, f);
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
	struct request_fixture *f = new_fixture(test);
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

static struct kunit_case cases[] = {
	KUNIT_CASE(scalar_entries_are_copied_in_order),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_request",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_LICENSE("GPL");
