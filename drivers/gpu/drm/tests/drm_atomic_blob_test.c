// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <drm/drm_kunit_helpers.h>
#include <drm/drm_property.h>
#include <kunit/test.h>

struct blob_fixture {
	struct drm_device *dev;
	struct drm_property_blob *value;
};

static void clear_value(void *data)
{
	struct blob_fixture *f = data;

	drm_property_blob_put(f->value);
}

static void put_blob(void *data)
{
	drm_property_blob_put(data);
}

static struct blob_fixture *new_fixture(struct kunit *test)
{
	struct blob_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct device *parent = drm_kunit_helper_alloc_device(test);

	KUNIT_ASSERT_NOT_NULL(test, f);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	f->dev = __drm_kunit_helper_alloc_drm_device(test, parent, sizeof(*f->dev), 0,
						  DRIVER_MODESET | DRIVER_ATOMIC);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->dev);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, clear_value, f), 0);
	return f;
}

static struct drm_property_blob *new_blob(struct kunit *test, struct drm_device *dev)
{
	struct drm_property_blob *blob = drm_property_create_blob(dev, 16, NULL);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, blob);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_blob, blob), 0);
	return blob;
}

static void destination_retains_resolved_blob(struct kunit *test)
{
	struct blob_fixture *f = new_fixture(test);
	struct drm_property_blob *blob = new_blob(test, f->dev);
	bool replaced = false;

	KUNIT_ASSERT_EQ(test, drm_property_replace_blob_checked(f->dev, &f->value, blob,
							      16, 16, 4, &replaced), 0);
	KUNIT_EXPECT_TRUE(test, replaced);
	KUNIT_EXPECT_PTR_EQ(test, f->value, blob);
	KUNIT_EXPECT_EQ(test, kref_read(&blob->base.refcount), 2);
	kunit_release_action(test, put_blob, blob);
	KUNIT_EXPECT_EQ(test, kref_read(&f->value->base.refcount), 1);
}

static void size_failure_preserves_destination(struct kunit *test)
{
	struct blob_fixture *f = new_fixture(test);
	struct drm_property_blob *old = new_blob(test, f->dev);
	struct drm_property_blob *next = new_blob(test, f->dev);
	const ssize_t limits[][3] = { { 8, -1, -1 }, { -1, 8, -1 }, { -1, -1, 3 } };
	unsigned int i;
	bool replaced = false;

	drm_property_replace_blob(&f->value, old);
	for (i = 0; i < ARRAY_SIZE(limits); i++) {
		KUNIT_EXPECT_EQ(test, drm_property_replace_blob_checked(f->dev, &f->value,
				 next, limits[i][0], limits[i][1], limits[i][2], &replaced),
				 -EINVAL);
		KUNIT_EXPECT_PTR_EQ(test, f->value, old);
		KUNIT_EXPECT_FALSE(test, replaced);
		KUNIT_EXPECT_EQ(test, kref_read(&old->base.refcount), 2);
		KUNIT_EXPECT_EQ(test, kref_read(&next->base.refcount), 1);
	}
}

static void same_blob_preserves_change_flag(struct kunit *test)
{
	struct blob_fixture *f = new_fixture(test);
	struct drm_property_blob *blob = new_blob(test, f->dev);
	bool replaced = false;

	drm_property_replace_blob(&f->value, blob);
	KUNIT_ASSERT_EQ(test, drm_property_replace_blob_checked(f->dev, &f->value, blob,
							      -1, -1, -1, &replaced), 0);
	KUNIT_EXPECT_FALSE(test, replaced);
	replaced = true;
	KUNIT_ASSERT_EQ(test, drm_property_replace_blob_checked(f->dev, &f->value, blob,
							      -1, -1, -1, &replaced), 0);
	KUNIT_EXPECT_TRUE(test, replaced);
	KUNIT_EXPECT_EQ(test, kref_read(&blob->base.refcount), 2);
}

static struct kunit_case blob_tests[] = {
	KUNIT_CASE(destination_retains_resolved_blob),
	KUNIT_CASE(size_failure_preserves_destination),
	KUNIT_CASE(same_blob_preserves_change_flag),
	{ }
};

static struct kunit_suite blob_suite = {
	.name = "drm_atomic_blob",
	.test_cases = blob_tests,
};

kunit_test_suite(blob_suite);
MODULE_LICENSE("GPL");
