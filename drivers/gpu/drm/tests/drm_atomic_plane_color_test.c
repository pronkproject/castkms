// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <drm/drm_atomic_uapi.h>
#include <drm/drm_color_mgmt.h>
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
	KUNIT_ASSERT_EQ(test, drm_plane_create_color_properties(state->plane,
		BIT(DRM_COLOR_YCBCR_BT601) | BIT(DRM_COLOR_YCBCR_BT709),
		BIT(DRM_COLOR_YCBCR_LIMITED_RANGE) | BIT(DRM_COLOR_YCBCR_FULL_RANGE),
		DRM_COLOR_YCBCR_BT601, DRM_COLOR_YCBCR_LIMITED_RANGE), 0);
	state->color_encoding = DRM_COLOR_YCBCR_BT601;
	state->color_range = DRM_COLOR_YCBCR_LIMITED_RANGE;
	return state;
}

static int set_color(struct drm_plane_state *state, struct drm_property *property, u64 value)
{
	int ret = drm_modeset_lock(&state->plane->mutex, NULL);

	if (ret)
		return ret;
	ret = drm_atomic_set_color_property_for_plane(state, property, value);
	drm_modeset_unlock(&state->plane->mutex);
	return ret;
}

static void encoding_changes_only_the_selected_field(struct kunit *test)
{
	struct drm_plane_state *state = new_state(test);

	KUNIT_ASSERT_EQ(test, set_color(state, state->plane->color_encoding_property,
				       DRM_COLOR_YCBCR_BT709), 0);
	KUNIT_EXPECT_EQ(test, state->color_encoding, DRM_COLOR_YCBCR_BT709);
	KUNIT_EXPECT_EQ(test, state->color_range, DRM_COLOR_YCBCR_LIMITED_RANGE);
}

static struct kunit_case cases[] = {
	KUNIT_CASE(encoding_changes_only_the_selected_field),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_plane_color",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_LICENSE("GPL");
