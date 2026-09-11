// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <drm/drm_atomic_uapi.h>
#include <drm/drm_color_mgmt.h>
#include <drm/drm_crtc.h>
#include <drm/drm_kunit_helpers.h>
#include <drm/drm_property.h>
#include <kunit/test.h>

struct color_fixture {
	struct drm_device *dev;
	struct drm_crtc_state state;
};

static int set_color(struct drm_crtc_state *state, struct drm_property *property,
		     struct drm_property_blob *blob)
{
	int ret = drm_modeset_lock(&state->crtc->mutex, NULL);

	if (ret)
		return ret;
	ret = drm_atomic_set_color_property_for_crtc(state, property, blob);
	drm_modeset_unlock(&state->crtc->mutex);
	return ret;
}

static void finish_fixture(void *data)
{
	struct color_fixture *f = data;

	drm_property_blob_put(f->state.degamma_lut);
	drm_property_blob_put(f->state.ctm);
	drm_property_blob_put(f->state.gamma_lut);
}

static struct color_fixture *new_fixture(struct kunit *test)
{
	struct color_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct device *parent = drm_kunit_helper_alloc_device(test);
	struct drm_plane *plane;

	KUNIT_ASSERT_NOT_NULL(test, f);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	f->dev = __drm_kunit_helper_alloc_drm_device(test, parent, sizeof(*f->dev), 0,
						  DRIVER_MODESET | DRIVER_ATOMIC);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->dev);
	plane = drm_kunit_helper_create_primary_plane(test, f->dev, NULL, NULL, NULL, 0, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, plane);
	f->state.crtc = drm_kunit_helper_create_crtc(test, f->dev, plane, NULL, NULL, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->state.crtc);
	drm_crtc_enable_color_mgmt(f->state.crtc, 2, true, 4);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, finish_fixture, f), 0);
	return f;
}

static void put_blob(void *data)
{
	drm_property_blob_put(data);
}

static struct drm_property_blob *new_blob(struct kunit *test, struct drm_device *dev,
					  size_t size)
{
	struct drm_property_blob *blob = drm_property_create_blob(dev, size, NULL);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, blob);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_blob, blob), 0);
	return blob;
}

static void color_stages_retain_their_blobs(struct kunit *test)
{
	struct color_fixture *f = new_fixture(test);
	struct drm_mode_config *config = &f->dev->mode_config;
	struct drm_property *properties[] = {
		config->degamma_lut_property, config->ctm_property, config->gamma_lut_property,
	};
	struct drm_property_blob **destinations[] = {
		&f->state.degamma_lut, &f->state.ctm, &f->state.gamma_lut,
	};
	size_t sizes[] = {
		2 * sizeof(struct drm_color_lut), sizeof(struct drm_color_ctm),
		4 * sizeof(struct drm_color_lut),
	};
	unsigned int i;

	for (i = 0; i < ARRAY_SIZE(properties); i++) {
		struct drm_property_blob *blob = new_blob(test, f->dev, sizes[i]);

		f->state.color_mgmt_changed = false;
		KUNIT_ASSERT_EQ(test, set_color(&f->state, properties[i], blob), 0);
		KUNIT_EXPECT_PTR_EQ(test, *destinations[i], blob);
		KUNIT_EXPECT_TRUE(test, f->state.color_mgmt_changed);
		KUNIT_EXPECT_EQ(test, kref_read(&blob->base.refcount), 2);
		kunit_release_action(test, put_blob, blob);
		KUNIT_EXPECT_EQ(test, kref_read(&(*destinations[i])->base.refcount), 1);
	}
}

static void invalid_color_sizes_preserve_state(struct kunit *test)
{
	struct color_fixture *f = new_fixture(test);
	struct drm_mode_config *config = &f->dev->mode_config;
	struct drm_property *properties[] = {
		config->degamma_lut_property, config->degamma_lut_property,
		config->gamma_lut_property, config->gamma_lut_property,
		config->ctm_property, config->ctm_property,
	};
	size_t sizes[] = {
		3 * sizeof(struct drm_color_lut), sizeof(struct drm_color_lut) + 1,
		5 * sizeof(struct drm_color_lut), sizeof(struct drm_color_lut) + 1,
		sizeof(struct drm_color_ctm) - 1, sizeof(struct drm_color_ctm) + 1,
	};
	unsigned int i;

	for (i = 0; i < ARRAY_SIZE(properties); i++) {
		struct drm_property_blob *blob = new_blob(test, f->dev, sizes[i]);

		KUNIT_EXPECT_EQ(test, set_color(&f->state, properties[i], blob), -EINVAL);
		KUNIT_EXPECT_FALSE(test, f->state.color_mgmt_changed);
		KUNIT_EXPECT_PTR_EQ(test, f->state.degamma_lut, NULL);
		KUNIT_EXPECT_PTR_EQ(test, f->state.ctm, NULL);
		KUNIT_EXPECT_PTR_EQ(test, f->state.gamma_lut, NULL);
		KUNIT_EXPECT_EQ(test, kref_read(&blob->base.refcount), 1);
	}
}

static void color_change_records_replacement(struct kunit *test)
{
	struct color_fixture *f = new_fixture(test);
	struct drm_property *property = f->dev->mode_config.gamma_lut_property;
	struct drm_property_blob *blob = new_blob(test, f->dev, sizeof(struct drm_color_lut));

	KUNIT_ASSERT_EQ(test, set_color(&f->state, property, blob), 0);
	f->state.color_mgmt_changed = false;
	KUNIT_ASSERT_EQ(test, set_color(&f->state, property, blob), 0);
	KUNIT_EXPECT_FALSE(test, f->state.color_mgmt_changed);
	f->state.color_mgmt_changed = true;
	KUNIT_ASSERT_EQ(test, set_color(&f->state, property, blob), 0);
	KUNIT_EXPECT_TRUE(test, f->state.color_mgmt_changed);
	f->state.color_mgmt_changed = false;
	KUNIT_ASSERT_EQ(test, set_color(&f->state, property, NULL), 0);
	KUNIT_EXPECT_TRUE(test, f->state.color_mgmt_changed);
	KUNIT_EXPECT_PTR_EQ(test, f->state.gamma_lut, NULL);
	KUNIT_EXPECT_EQ(test, kref_read(&blob->base.refcount), 1);
}

static void color_property_must_be_attached(struct kunit *test)
{
	struct color_fixture *f = new_fixture(test);
	struct drm_object_properties *properties = f->state.crtc->base.properties;
	int count = properties->count;
	int ret;

	properties->count = 0;
	ret = set_color(&f->state, f->dev->mode_config.ctm_property, NULL);
	properties->count = count;
	KUNIT_EXPECT_EQ(test, ret, -EINVAL);
	KUNIT_EXPECT_FALSE(test, f->state.color_mgmt_changed);
}

static void advertised_table_size_must_bound_bytes(struct kunit *test)
{
	struct color_fixture *f = new_fixture(test);
	struct drm_mode_config *config = &f->dev->mode_config;
	struct drm_property_blob *blob = new_blob(test, f->dev, sizeof(struct drm_color_lut));

	KUNIT_ASSERT_EQ(test, drm_object_property_set_value(&f->state.crtc->base,
							  config->gamma_lut_size_property, U64_MAX), 0);
	KUNIT_EXPECT_EQ(test, set_color(&f->state, config->gamma_lut_property, blob),
			-EOVERFLOW);
	KUNIT_ASSERT_EQ(test, drm_object_property_set_value(&f->state.crtc->base,
							  config->gamma_lut_size_property, 0), 0);
	KUNIT_EXPECT_EQ(test, set_color(&f->state, config->gamma_lut_property, blob), -EINVAL);
	KUNIT_EXPECT_PTR_EQ(test, f->state.gamma_lut, NULL);
	KUNIT_EXPECT_FALSE(test, f->state.color_mgmt_changed);
	KUNIT_EXPECT_EQ(test, kref_read(&blob->base.refcount), 1);
}

static struct kunit_case color_tests[] = {
	KUNIT_CASE(color_stages_retain_their_blobs),
	KUNIT_CASE(invalid_color_sizes_preserve_state),
	KUNIT_CASE(color_change_records_replacement),
	KUNIT_CASE(color_property_must_be_attached),
	KUNIT_CASE(advertised_table_size_must_bound_bytes),
	{ }
};

static struct kunit_suite color_suite = {
	.name = "drm_atomic_color",
	.test_cases = color_tests,
};

kunit_test_suite(color_suite);
MODULE_LICENSE("GPL");
