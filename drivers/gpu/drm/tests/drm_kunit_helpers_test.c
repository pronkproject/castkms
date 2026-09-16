// SPDX-License-Identifier: GPL-2.0

#include <linux/module.h>
#include <drm/drm_fourcc.h>
#include <drm/drm_kunit_helpers.h>
#include <drm/drm_plane.h>
#include <kunit/test.h>

struct helper_fixture {
	struct drm_device drm;
};

static struct drm_device *new_device(struct kunit *test)
{
	struct device *dev = drm_kunit_helper_alloc_device(test);
	struct helper_fixture *fixture;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, dev);
	fixture = drm_kunit_helper_alloc_drm_device(test, dev, struct helper_fixture, drm,
						   DRIVER_MODESET | DRIVER_ATOMIC);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, fixture);
	return &fixture->drm;
}

static void primary_plane_retains_supplied_modifiers(struct kunit *test)
{
	static const u64 modifiers[] = {
		DRM_FORMAT_MOD_LINEAR, I915_FORMAT_MOD_X_TILED, DRM_FORMAT_MOD_INVALID,
	};
	struct drm_device *dev = new_device(test);
	struct drm_plane *plane = drm_kunit_helper_create_primary_plane(test, dev,
							NULL, NULL, NULL, 0, modifiers);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, plane);
	KUNIT_ASSERT_EQ(test, plane->modifier_count, 2);
	KUNIT_EXPECT_EQ(test, plane->modifiers[1], I915_FORMAT_MOD_X_TILED);
	KUNIT_EXPECT_TRUE(test, drm_plane_has_format(plane, DRM_FORMAT_XRGB8888,
						    I915_FORMAT_MOD_X_TILED));
}

static void primary_plane_defaults_to_linear(struct kunit *test)
{
	struct drm_device *dev = new_device(test);
	struct drm_plane *plane = drm_kunit_helper_create_primary_plane(test, dev,
							NULL, NULL, NULL, 0, NULL);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, plane);
	KUNIT_ASSERT_EQ(test, plane->modifier_count, 1);
	KUNIT_EXPECT_EQ(test, plane->modifiers[0], DRM_FORMAT_MOD_LINEAR);
}

static struct kunit_case drm_kunit_helpers_tests[] = {
	KUNIT_CASE(primary_plane_retains_supplied_modifiers),
	KUNIT_CASE(primary_plane_defaults_to_linear),
	{}
};

static struct kunit_suite drm_kunit_helpers_test_suite = {
	.name = "drm_kunit_helpers",
	.test_cases = drm_kunit_helpers_tests,
};

kunit_test_suite(drm_kunit_helpers_test_suite);

MODULE_LICENSE("GPL");
