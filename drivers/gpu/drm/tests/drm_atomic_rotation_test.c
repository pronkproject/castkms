// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <drm/drm_atomic_uapi.h>
#include <drm/drm_blend.h>
#include <drm/drm_kunit_helpers.h>
#include <drm/drm_modeset_lock.h>
#include <drm/drm_plane.h>
#include <kunit/test.h>

static struct drm_plane_state *new_state(struct kunit *test, bool with_property)
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
	state->rotation = DRM_MODE_ROTATE_0;
	if (with_property)
		KUNIT_ASSERT_EQ(test, drm_plane_create_rotation_property(state->plane,
			DRM_MODE_ROTATE_0, DRM_MODE_ROTATE_0 | DRM_MODE_ROTATE_90 | DRM_MODE_REFLECT_X), 0);
	return state;
}

static int set_rotation(struct drm_plane_state *state, u64 rotation)
{
	int ret = drm_modeset_lock(&state->plane->mutex, NULL);

	if (ret)
		return ret;
	ret = drm_atomic_set_rotation_for_plane(state, rotation);
	drm_modeset_unlock(&state->plane->mutex);
	return ret;
}

static void rotation_accepts_advertised_reflection(struct kunit *test)
{
	struct drm_plane_state *state = new_state(test, true);
	u32 rotation = DRM_MODE_ROTATE_90 | DRM_MODE_REFLECT_X;

	KUNIT_ASSERT_EQ(test, set_rotation(state, rotation), 0);
	KUNIT_EXPECT_EQ(test, state->rotation, rotation);
}

static void rotation_requires_exactly_one_angle(struct kunit *test)
{
	struct drm_plane_state *state = new_state(test, true);

	KUNIT_EXPECT_EQ(test, set_rotation(state, DRM_MODE_REFLECT_X), -EINVAL);
	KUNIT_EXPECT_EQ(test, state->rotation, DRM_MODE_ROTATE_0);
	KUNIT_EXPECT_EQ(test, set_rotation(state, DRM_MODE_ROTATE_0 | DRM_MODE_ROTATE_90), -EINVAL);
	KUNIT_EXPECT_EQ(test, state->rotation, DRM_MODE_ROTATE_0);
}

static struct kunit_case cases[] = {
	KUNIT_CASE(rotation_accepts_advertised_reflection),
	KUNIT_CASE(rotation_requires_exactly_one_angle),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_rotation",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_LICENSE("GPL");
