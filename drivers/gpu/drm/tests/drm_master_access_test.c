// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <drm/drm_auth.h>
#include <drm/drm_drv.h>
#include <drm/drm_kunit_helpers.h>
#include <drm/drm_lease.h>
#include <drm/drm_plane.h>
#include <kunit/test.h>

static const struct drm_driver test_driver = {
	.driver_features = DRIVER_MODESET | DRIVER_ATOMIC,
};

struct access_fixture {
	struct drm_device *dev;
	struct drm_master *root;
	struct drm_plane *plane;
};

static void put_master(void *data)
{
	struct drm_master *master = data;

	drm_master_put(&master);
}

static void clear_current(void *data)
{
	struct drm_device *dev = data;
	struct drm_master *old;

	mutex_lock(&dev->master_mutex);
	old = dev->master;
	dev->master = NULL;
	mutex_unlock(&dev->master_mutex);
	if (old)
		drm_master_put(&old);
}

static struct drm_master *new_master(struct kunit *test, struct drm_device *dev,
				     struct drm_master *lessor)
{
	struct drm_master *master = drm_master_create(dev);

	KUNIT_ASSERT_NOT_NULL(test, master);
	if (lessor) {
		guard(mutex)(&dev->mode_config.idr_mutex);
		master->lessor = drm_master_get(lessor);
		list_add_tail(&master->lessee_list, &lessor->lessees);
	}
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_master, master), 0);
	return master;
}

static struct access_fixture *new_fixture(struct kunit *test)
{
	struct access_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct device *parent = drm_kunit_helper_alloc_device(test);

	KUNIT_ASSERT_NOT_NULL(test, f);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	f->dev = __drm_kunit_helper_alloc_drm_device_with_driver(test, parent,
								 sizeof(*f->dev), 0, &test_driver);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->dev);
	f->plane = drm_kunit_helper_create_primary_plane(test, f->dev, NULL, NULL, NULL, 0, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->plane);
	f->root = new_master(test, f->dev, NULL);
	mutex_lock(&f->dev->master_mutex);
	f->dev->master = drm_master_get(f->root);
	mutex_unlock(&f->dev->master_mutex);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, clear_current, f->dev), 0);
	return f;
}

static bool holds_plane(struct drm_master *master, struct drm_plane *plane)
{
	bool held;

	if (!drm_master_lock_current(master))
		return false;
	held = drm_master_holds_object_locked(master, &plane->base);
	drm_master_unlock_current(master);
	return held;
}

static void current_root_controls_registered_object(struct kunit *test)
{
	struct access_fixture *f = new_fixture(test);

	KUNIT_EXPECT_TRUE(test, holds_plane(f->root, f->plane));
	KUNIT_EXPECT_TRUE(test, holds_plane(f->root, f->plane));
}

static void retained_root_loses_current_access(struct kunit *test)
{
	struct access_fixture *f = new_fixture(test);
	struct drm_master *replacement = new_master(test, f->dev, NULL);

	clear_current(f->dev);
	KUNIT_EXPECT_FALSE(test, holds_plane(f->root, f->plane));
	mutex_lock(&f->dev->master_mutex);
	f->dev->master = drm_master_get(replacement);
	mutex_unlock(&f->dev->master_mutex);
	KUNIT_EXPECT_FALSE(test, holds_plane(f->root, f->plane));
	KUNIT_EXPECT_TRUE(test, holds_plane(replacement, f->plane));
}

static void lease_revocation_changes_object_access(struct kunit *test)
{
	struct access_fixture *f = new_fixture(test);
	struct drm_master *lessee = new_master(test, f->dev, f->root);
	int ret;

	KUNIT_EXPECT_FALSE(test, holds_plane(lessee, f->plane));
	mutex_lock(&f->dev->mode_config.idr_mutex);
	ret = idr_alloc(&lessee->leases, f->plane, f->plane->base.id,
			f->plane->base.id + 1, GFP_KERNEL);
	mutex_unlock(&f->dev->mode_config.idr_mutex);
	KUNIT_ASSERT_GE(test, ret, 0);
	KUNIT_EXPECT_TRUE(test, holds_plane(lessee, f->plane));
	drm_lease_revoke(lessee);
	KUNIT_EXPECT_FALSE(test, holds_plane(lessee, f->plane));
	KUNIT_EXPECT_TRUE(test, holds_plane(f->root, f->plane));
}

static void retained_lease_loses_current_root(struct kunit *test)
{
	struct access_fixture *f = new_fixture(test);
	struct drm_master *lessee = new_master(test, f->dev, f->root);
	bool acquired;

	clear_current(f->dev);
	acquired = drm_master_lock_current(lessee);
	if (acquired)
		drm_master_unlock_current(lessee);
	KUNIT_EXPECT_FALSE(test, acquired);
}

static void registration_requires_the_same_object(struct kunit *test)
{
	struct access_fixture *f = new_fixture(test);
	struct drm_mode_object replacement = { .id = f->plane->base.id };
	void *old;
	bool absent, replaced;

	KUNIT_ASSERT_TRUE(test, drm_master_lock_current(f->root));
	old = idr_replace(&f->dev->mode_config.object_idr, NULL, replacement.id);
	absent = drm_master_holds_object_locked(f->root, &f->plane->base);
	idr_replace(&f->dev->mode_config.object_idr, &replacement, replacement.id);
	replaced = drm_master_holds_object_locked(f->root, &f->plane->base);
	idr_replace(&f->dev->mode_config.object_idr, old, replacement.id);
	drm_master_unlock_current(f->root);
	KUNIT_EXPECT_PTR_EQ(test, old, &f->plane->base);
	KUNIT_EXPECT_FALSE(test, absent);
	KUNIT_EXPECT_FALSE(test, replaced);
	KUNIT_EXPECT_TRUE(test, holds_plane(f->root, f->plane));
}

static struct kunit_case cases[] = {
	KUNIT_CASE(current_root_controls_registered_object),
	KUNIT_CASE(retained_root_loses_current_access),
	KUNIT_CASE(lease_revocation_changes_object_access),
	KUNIT_CASE(retained_lease_loses_current_root),
	KUNIT_CASE(registration_requires_the_same_object),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_master_access",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_IMPORT_NS("EXPORTED_FOR_KUNIT_TESTING");
MODULE_DESCRIPTION("DRM current master and object access tests");
MODULE_LICENSE("GPL");
