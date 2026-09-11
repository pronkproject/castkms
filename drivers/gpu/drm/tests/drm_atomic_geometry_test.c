// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <drm/drm_atomic_uapi.h>
#include <drm/drm_kunit_helpers.h>
#include <drm/drm_plane.h>
#include <drm/drm_property.h>
#include <kunit/test.h>

struct geometry_fixture {
	struct drm_device *dev;
	struct drm_plane_state state;
};

static int set_geometry(struct drm_plane_state *state, struct drm_property *property, u64 value)
{
	int ret = drm_modeset_lock(&state->plane->mutex, NULL);

	if (ret)
		return ret;
	ret = drm_atomic_set_geometry_property_for_plane(state, property, value);
	drm_modeset_unlock(&state->plane->mutex);
	return ret;
}

static struct geometry_fixture *new_fixture(struct kunit *test)
{
	struct geometry_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct device *parent = drm_kunit_helper_alloc_device(test);

	KUNIT_ASSERT_NOT_NULL(test, f);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	f->dev = __drm_kunit_helper_alloc_drm_device(test, parent, sizeof(*f->dev), 0,
						  DRIVER_MODESET | DRIVER_ATOMIC);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->dev);
	f->state.plane = drm_kunit_helper_create_primary_plane(test, f->dev, NULL, NULL,
							       NULL, 0, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->state.plane);
	return f;
}

static void coordinates_keep_their_representation(struct kunit *test)
{
	struct geometry_fixture *f = new_fixture(test);
	struct drm_mode_config *config = &f->dev->mode_config;
	struct drm_plane_state *state = &f->state;

	KUNIT_ASSERT_EQ(test, set_geometry(state, config->prop_crtc_x, (u64)-7), 0);
	KUNIT_ASSERT_EQ(test, set_geometry(state, config->prop_crtc_y, (u64)-11), 0);
	KUNIT_ASSERT_EQ(test, set_geometry(state, config->prop_crtc_w, 1920), 0);
	KUNIT_ASSERT_EQ(test, set_geometry(state, config->prop_crtc_h, 1080), 0);
	KUNIT_ASSERT_EQ(test, set_geometry(state, config->prop_src_x, 0x18000), 0);
	KUNIT_ASSERT_EQ(test, set_geometry(state, config->prop_src_y, 0x24000), 0);
	KUNIT_ASSERT_EQ(test, set_geometry(state, config->prop_src_w, 640 << 16), 0);
	KUNIT_ASSERT_EQ(test, set_geometry(state, config->prop_src_h, 480 << 16), 0);
	KUNIT_EXPECT_EQ(test, state->crtc_x, -7);
	KUNIT_EXPECT_EQ(test, state->crtc_y, -11);
	KUNIT_EXPECT_EQ(test, state->crtc_w, 1920);
	KUNIT_EXPECT_EQ(test, state->crtc_h, 1080);
	KUNIT_EXPECT_EQ(test, state->src_x, 0x18000);
	KUNIT_EXPECT_EQ(test, state->src_y, 0x24000);
	KUNIT_EXPECT_EQ(test, state->src_w, 640 << 16);
	KUNIT_EXPECT_EQ(test, state->src_h, 480 << 16);
}

static void invalid_values_leave_geometry_unchanged(struct kunit *test)
{
	struct geometry_fixture *f = new_fixture(test);
	struct drm_mode_config *config = &f->dev->mode_config;
	struct drm_plane_state *state = &f->state;

	state->crtc_x = 17;
	state->crtc_w = 19;
	state->src_x = 23;
	KUNIT_EXPECT_EQ(test, set_geometry(state, config->prop_crtc_x, (u64)INT_MAX + 1), -EINVAL);
	KUNIT_EXPECT_EQ(test, set_geometry(state, config->prop_crtc_x, (u64)((s64)INT_MIN - 1)), -EINVAL);
	KUNIT_EXPECT_EQ(test, set_geometry(state, config->prop_crtc_w, U64_MAX), -EINVAL);
	KUNIT_EXPECT_EQ(test, set_geometry(state, config->prop_src_x, (u64)U32_MAX + 1), -EINVAL);
	KUNIT_EXPECT_EQ(test, state->crtc_x, 17);
	KUNIT_EXPECT_EQ(test, state->crtc_w, 19);
	KUNIT_EXPECT_EQ(test, state->src_x, 23);
}

static void property_names_do_not_select_geometry(struct kunit *test)
{
	struct geometry_fixture *f = new_fixture(test);
	struct drm_property *property = drm_property_create_range(f->dev, DRM_MODE_PROP_ATOMIC,
									"CRTC_X", 0, 100);

	KUNIT_ASSERT_NOT_NULL(test, property);
	drm_object_attach_property(&f->state.plane->base, property, 0);
	f->state.crtc_x = 17;
	KUNIT_EXPECT_EQ(test, set_geometry(&f->state, property, 42),
			-EOPNOTSUPP);
	KUNIT_EXPECT_EQ(test, f->state.crtc_x, 17);
}

static void geometry_property_must_be_attached(struct kunit *test)
{
	struct geometry_fixture *f = new_fixture(test);
	struct drm_object_properties *properties = f->state.plane->base.properties;
	int count = properties->count;
	int ret;

	f->state.crtc_x = 17;
	properties->count = 0;
	ret = set_geometry(&f->state, f->dev->mode_config.prop_crtc_x, 42);
	properties->count = count;
	KUNIT_EXPECT_EQ(test, ret, -EINVAL);
	KUNIT_EXPECT_EQ(test, f->state.crtc_x, 17);
}

static struct kunit_case geometry_tests[] = {
	KUNIT_CASE(coordinates_keep_their_representation),
	KUNIT_CASE(invalid_values_leave_geometry_unchanged),
	KUNIT_CASE(property_names_do_not_select_geometry),
	KUNIT_CASE(geometry_property_must_be_attached),
	{ }
};

static struct kunit_suite geometry_suite = {
	.name = "drm_atomic_geometry",
	.test_cases = geometry_tests,
};

kunit_test_suite(geometry_suite);
MODULE_LICENSE("GPL");
