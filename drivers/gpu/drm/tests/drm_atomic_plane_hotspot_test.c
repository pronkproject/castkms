// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <drm/drm_atomic_uapi.h>
#include <drm/drm_kunit_helpers.h>
#include <drm/drm_modeset_lock.h>
#include <drm/drm_plane.h>
#include <drm/drm_property.h>
#include <kunit/test.h>

static struct drm_plane_state *new_state(struct kunit *test)
{
	struct drm_plane_state *state = kunit_kzalloc(test, sizeof(*state), GFP_KERNEL);
	struct device *parent = drm_kunit_helper_alloc_device(test);
	struct drm_device *dev;

	KUNIT_ASSERT_NOT_NULL(test, state);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	dev = __drm_kunit_helper_alloc_drm_device(test, parent, sizeof(*dev), 0,
						  DRIVER_MODESET | DRIVER_ATOMIC);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, dev);
	state->plane = drm_kunit_helper_create_primary_plane(test, dev, NULL, NULL, NULL, 0, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state->plane);
	state->plane->type = DRM_PLANE_TYPE_CURSOR;
	state->plane->hotspot_x_property =
		drm_property_create_signed_range(dev, 0, "HOTSPOT_X", INT_MIN, INT_MAX);
	state->plane->hotspot_y_property =
		drm_property_create_signed_range(dev, 0, "HOTSPOT_Y", INT_MIN, INT_MAX);
	KUNIT_ASSERT_NOT_NULL(test, state->plane->hotspot_x_property);
	KUNIT_ASSERT_NOT_NULL(test, state->plane->hotspot_y_property);
	drm_object_attach_property(&state->plane->base, state->plane->hotspot_x_property, 0);
	drm_object_attach_property(&state->plane->base, state->plane->hotspot_y_property, 0);
	return state;
}

static int set_hotspot(struct drm_plane_state *state, struct drm_property *property, u64 value)
{
	int ret = drm_modeset_lock(&state->plane->mutex, NULL);

	if (ret)
		return ret;
	ret = drm_atomic_set_hotspot_property_for_plane(state, property, value);
	drm_modeset_unlock(&state->plane->mutex);
	return ret;
}

static void signed_coordinates_change_independently(struct kunit *test)
{
	struct drm_plane_state *state = new_state(test);

	KUNIT_ASSERT_EQ(test, set_hotspot(state, state->plane->hotspot_x_property, -17), 0);
	KUNIT_EXPECT_EQ(test, state->hotspot_x, -17);
	KUNIT_EXPECT_EQ(test, state->hotspot_y, 0);
	KUNIT_ASSERT_EQ(test, set_hotspot(state, state->plane->hotspot_y_property, 23), 0);
	KUNIT_EXPECT_EQ(test, state->hotspot_x, -17);
	KUNIT_EXPECT_EQ(test, state->hotspot_y, 23);
}

static void invalid_values_leave_coordinates_unchanged(struct kunit *test)
{
	struct drm_plane_state *state = new_state(test);

	state->hotspot_x = 5;
	state->hotspot_y = -7;
	KUNIT_EXPECT_EQ(test,
			set_hotspot(state, state->plane->hotspot_x_property,
				    (u64)INT_MAX + 1),
			-EINVAL);
	KUNIT_EXPECT_EQ(test, state->hotspot_x, 5);
	KUNIT_EXPECT_EQ(test, state->hotspot_y, -7);
}

static void hotspot_requires_cursor_and_attachment(struct kunit *test)
{
	struct drm_plane_state *state = new_state(test);
	struct drm_object_properties *properties = state->plane->base.properties;
	unsigned int count = properties->count;
	int ret;

	state->plane->type = DRM_PLANE_TYPE_OVERLAY;
	KUNIT_EXPECT_EQ(test, set_hotspot(state, state->plane->hotspot_x_property, 1), -EINVAL);
	state->plane->type = DRM_PLANE_TYPE_CURSOR;
	properties->count = 0;
	ret = set_hotspot(state, state->plane->hotspot_y_property, 1);
	properties->count = count;
	KUNIT_EXPECT_EQ(test, ret, -EINVAL);
	KUNIT_EXPECT_EQ(test, state->hotspot_x, 0);
	KUNIT_EXPECT_EQ(test, state->hotspot_y, 0);
}

static void hotspot_requires_property_identity(struct kunit *test)
{
	struct drm_plane_state *state = new_state(test);
	struct drm_property *property = drm_property_create_signed_range(state->plane->dev, 0,
								       "HOTSPOT_X", INT_MIN,
								       INT_MAX);

	KUNIT_ASSERT_NOT_NULL(test, property);
	drm_object_attach_property(&state->plane->base, property, 0);
	KUNIT_EXPECT_EQ(test, set_hotspot(state, property, 1), -EOPNOTSUPP);
	KUNIT_EXPECT_EQ(test, set_hotspot(state, NULL, 1), -EOPNOTSUPP);
}

static struct kunit_case cases[] = {
	KUNIT_CASE(signed_coordinates_change_independently),
	KUNIT_CASE(invalid_values_leave_coordinates_unchanged),
	KUNIT_CASE(hotspot_requires_cursor_and_attachment),
	KUNIT_CASE(hotspot_requires_property_identity),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_plane_hotspot",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_LICENSE("GPL");
