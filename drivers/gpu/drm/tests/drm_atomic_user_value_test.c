// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <drm/drm_crtc.h>
#include <drm/drm_fourcc.h>
#include <drm/drm_framebuffer.h>
#include <drm/drm_kunit_helpers.h>
#include <drm/drm_property.h>
#include <kunit/test.h>

#include "../drm_atomic_user_value.h"

struct value_fixture {
	struct drm_device *dev;
	struct drm_plane *plane;
	struct drm_crtc *crtc;
};

static struct value_fixture *new_fixture(struct kunit *test)
{
	struct value_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct device *parent = drm_kunit_helper_alloc_device(test);

	KUNIT_ASSERT_NOT_NULL(test, f);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	f->dev = __drm_kunit_helper_alloc_drm_device(test, parent, sizeof(*f->dev), 0,
						  DRIVER_MODESET | DRIVER_ATOMIC);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->dev);
	f->plane = drm_kunit_helper_create_primary_plane(test, f->dev, NULL, NULL, NULL, 0, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->plane);
	f->crtc = drm_kunit_helper_create_crtc(test, f->dev, f->plane, NULL, NULL, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->crtc);
	return f;
}

static void release_value(void *data)
{
	drm_atomic_release_user_value(data);
}

static struct drm_atomic_request_entry *resolve_value(struct kunit *test,
		struct drm_mode_object *object, struct drm_property *property, u64 value)
{
	struct drm_atomic_request_entry *entry = kunit_kzalloc(test, sizeof(*entry), GFP_KERNEL);

	KUNIT_ASSERT_NOT_NULL(test, entry);
	KUNIT_ASSERT_EQ(test, drm_atomic_resolve_user_value(object, property, NULL, value, entry), 0);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, release_value, entry), 0);
	return entry;
}

static void scalar_value_preserves_signed_bits(struct kunit *test)
{
	struct value_fixture *f = new_fixture(test);
	struct drm_atomic_request_entry *entry = resolve_value(test, &f->plane->base,
							f->dev->mode_config.prop_crtc_x, (u64)-7);

	KUNIT_EXPECT_PTR_EQ(test, entry->object, &f->plane->base);
	KUNIT_EXPECT_EQ(test, entry->type, DRM_ATOMIC_REQUEST_SCALAR);
	KUNIT_EXPECT_EQ(test, entry->scalar, (u64)-7);
}

static void invalid_value_preserves_destination(struct kunit *test)
{
	struct value_fixture *f = new_fixture(test);
	struct drm_atomic_request_entry entry = { .scalar = 23 };

	KUNIT_EXPECT_EQ(test, drm_atomic_resolve_user_value(&f->crtc->base,
			f->dev->mode_config.prop_active, NULL, 2, &entry), -EINVAL);
	KUNIT_EXPECT_PTR_EQ(test, entry.object, NULL);
	KUNIT_EXPECT_EQ(test, entry.scalar, 23);
}

static void private_scalar_is_not_resolved(struct kunit *test)
{
	struct value_fixture *f = new_fixture(test);
	struct drm_property *property = drm_property_create_range(f->dev, DRM_MODE_PROP_ATOMIC,
									"private_handle", 0, U32_MAX);
	struct drm_atomic_request_entry entry = { .scalar = 23 };

	KUNIT_ASSERT_NOT_NULL(test, property);
	drm_object_attach_property(&f->plane->base, property, 0);
	KUNIT_EXPECT_EQ(test, drm_atomic_resolve_user_value(&f->plane->base, property,
								 NULL, 42, &entry), -EOPNOTSUPP);
	KUNIT_EXPECT_EQ(test, entry.scalar, 23);
}

static void unattached_property_is_not_resolved(struct kunit *test)
{
	struct value_fixture *f = new_fixture(test);
	struct drm_atomic_request_entry entry = { .scalar = 23 };

	KUNIT_EXPECT_EQ(test, drm_atomic_resolve_user_value(&f->plane->base,
			f->dev->mode_config.prop_active, NULL, 1, &entry), -EINVAL);
	KUNIT_EXPECT_EQ(test, entry.scalar, 23);
}

static struct kunit_case cases[] = {
	KUNIT_CASE(scalar_value_preserves_signed_bits),
	KUNIT_CASE(invalid_value_preserves_destination),
	KUNIT_CASE(private_scalar_is_not_resolved),
	KUNIT_CASE(unattached_property_is_not_resolved),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_user_value",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_IMPORT_NS("EXPORTED_FOR_KUNIT_TESTING");
MODULE_LICENSE("GPL");
