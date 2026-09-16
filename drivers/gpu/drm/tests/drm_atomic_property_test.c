// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <drm/drm_atomic.h>
#include <drm/drm_atomic_uapi.h>
#include <drm/drm_blend.h>
#include <drm/drm_kunit_helpers.h>
#include <drm/drm_modeset_helper.h>
#include <drm/drm_property.h>
#include <kunit/device.h>
#include <kunit/test.h>

struct property_fixture {
	struct drm_device *dev;
	struct drm_plane *plane;
	struct drm_crtc *crtc;
	struct drm_atomic_commit *state;
};

static int read_property(struct property_fixture *f, struct drm_mode_object *object,
			 struct drm_property *property, u64 *value)
{
	struct drm_modeset_lock *lock = object->type == DRM_MODE_OBJECT_CRTC ?
		&f->crtc->mutex : &f->plane->mutex;
	int ret = drm_modeset_lock(lock, NULL);

	if (ret)
		return ret;
	ret = drm_atomic_get_property_from_state(f->state, object, property, value);
	drm_modeset_unlock(lock);
	return ret;
}

static struct property_fixture *new_fixture(struct kunit *test)
{
	struct property_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct device *parent = drm_kunit_helper_alloc_device(test);
	struct drm_modeset_acquire_ctx ctx;
	struct drm_plane_state *plane;
	struct drm_crtc_state *crtc;
	int ret;

	KUNIT_ASSERT_NOT_NULL(test, f);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	f->dev = __drm_kunit_helper_alloc_drm_device(test, parent, sizeof(*f->dev), 0,
						    DRIVER_MODESET | DRIVER_ATOMIC);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->dev);
	f->plane = drm_kunit_helper_create_primary_plane(test, f->dev,
							 NULL, NULL, NULL, 0, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->plane);
	KUNIT_ASSERT_EQ(test, drm_plane_create_alpha_property(f->plane), 0);
	f->crtc = drm_kunit_helper_create_crtc(test, f->dev, f->plane, NULL, NULL, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->crtc);
	drm_mode_config_reset(f->dev);
	f->state = drm_kunit_helper_atomic_state_alloc(test, f->dev, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->state);
	drm_modeset_acquire_init(&ctx, 0);
	f->state->acquire_ctx = &ctx;
	for (;;) {
		ret = drm_modeset_lock_all_ctx(f->dev, &ctx);
		if (ret != -EDEADLK)
			break;
		ret = drm_modeset_backoff(&ctx);
		if (ret)
			break;
	}
	if (!ret) {
		plane = drm_atomic_get_plane_state(f->state, f->plane);
		crtc = drm_atomic_get_crtc_state(f->state, f->crtc);
		ret = IS_ERR(plane) ? PTR_ERR(plane) : IS_ERR(crtc) ? PTR_ERR(crtc) : 0;
	}
	drm_modeset_drop_locks(&ctx);
	drm_modeset_acquire_fini(&ctx);
	f->state->acquire_ctx = NULL;
	KUNIT_ASSERT_EQ(test, ret, 0);
	return f;
}

static void reads_proposed_values_without_changing_accepted_state(struct kunit *test)
{
	struct property_fixture *f = new_fixture(test);
	struct drm_plane_state *plane = drm_atomic_get_new_plane_state(f->state, f->plane);
	struct drm_crtc_state *crtc = drm_atomic_get_new_crtc_state(f->state, f->crtc);
	u64 value = 0;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, plane);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, crtc);
	plane->alpha = 1234;
	plane->crtc_x = -7;
	crtc->vrr_enabled = true;
	KUNIT_ASSERT_EQ(test, read_property(f, &f->plane->base,
					   f->plane->alpha_property, &value), 0);
	KUNIT_EXPECT_EQ(test, value, 1234);
	KUNIT_EXPECT_NE(test, f->plane->state->alpha, 1234);
	KUNIT_ASSERT_EQ(test, read_property(f, &f->plane->base,
					   f->dev->mode_config.prop_crtc_x, &value), 0);
	KUNIT_EXPECT_EQ(test, value, (u64)-7);
	KUNIT_ASSERT_EQ(test, read_property(f, &f->crtc->base,
					   f->dev->mode_config.prop_vrr_enabled, &value), 0);
	KUNIT_EXPECT_EQ(test, value, 1);
	KUNIT_EXPECT_FALSE(test, f->crtc->state->vrr_enabled);
}

static void omitted_state_does_not_fall_back_to_current_values(struct kunit *test)
{
	struct property_fixture *f = new_fixture(test);
	u64 value = 123;

	drm_atomic_commit_clear(f->state);
	KUNIT_EXPECT_EQ(test, read_property(f, &f->plane->base,
					   f->plane->alpha_property, &value), -ENOENT);
	KUNIT_EXPECT_EQ(test, value, 123);
	KUNIT_EXPECT_EQ(test, read_property(f, &f->crtc->base,
					   f->dev->mode_config.prop_vrr_enabled, &value), -ENOENT);
	KUNIT_EXPECT_EQ(test, value, 123);
	KUNIT_EXPECT_PTR_EQ(test, drm_atomic_get_new_plane_state(f->state, f->plane), NULL);
	KUNIT_EXPECT_PTR_EQ(test, drm_atomic_get_new_crtc_state(f->state, f->crtc), NULL);
}

static void requires_attached_mutable_property_identity(struct kunit *test)
{
	struct property_fixture *f = new_fixture(test);
	struct drm_property *alias;
	u64 value = 123;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, drm_atomic_get_new_plane_state(f->state, f->plane));
	KUNIT_EXPECT_EQ(test, read_property(f, &f->plane->base,
					   f->dev->mode_config.prop_active, &value),
			-EINVAL);
	alias = drm_property_create_range(f->dev, DRM_MODE_PROP_ATOMIC, "alpha", 0, 65535);
	KUNIT_ASSERT_NOT_NULL(test, alias);
	drm_object_attach_property(&f->plane->base, alias, 0);
	KUNIT_EXPECT_EQ(test,
		read_property(f, &f->plane->base, alias, &value), -EINVAL);
	alias->flags |= DRM_MODE_PROP_IMMUTABLE;
	KUNIT_EXPECT_EQ(test,
		read_property(f, &f->plane->base, alias, &value), -EINVAL);
	KUNIT_EXPECT_EQ(test, value, 123);
}

static void rejects_foreign_device_before_indexing_state(struct kunit *test)
{
	struct property_fixture *f = new_fixture(test);
	struct device *parent = kunit_device_register(test, "foreign-property");
	struct drm_device *foreign;
	struct drm_plane *plane;
	u64 value = 123;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	foreign = __drm_kunit_helper_alloc_drm_device(test, parent, sizeof(*foreign), 0,
						      DRIVER_MODESET | DRIVER_ATOMIC);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, foreign);
	plane = drm_kunit_helper_create_primary_plane(test, foreign, NULL, NULL, NULL, 0, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, plane);
	KUNIT_EXPECT_EQ(test, drm_atomic_get_property_from_state(f->state, &plane->base,
							foreign->mode_config.prop_crtc_x, &value),
			-EXDEV);
	KUNIT_EXPECT_EQ(test, value, 123);
}

static struct kunit_case drm_atomic_property_tests[] = {
	KUNIT_CASE(reads_proposed_values_without_changing_accepted_state),
	KUNIT_CASE(omitted_state_does_not_fall_back_to_current_values),
	KUNIT_CASE(requires_attached_mutable_property_identity),
	KUNIT_CASE(rejects_foreign_device_before_indexing_state),
	{}
};

static struct kunit_suite drm_atomic_property_test_suite = {
	.name = "drm_atomic_property",
	.test_cases = drm_atomic_property_tests,
};

kunit_test_suite(drm_atomic_property_test_suite);

MODULE_DESCRIPTION("DRM proposed atomic property decoding tests");
MODULE_LICENSE("Dual MIT/GPL");
