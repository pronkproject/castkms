// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/file.h>
#include <drm/drm_auth.h>
#include <drm/drm_constraints_device.h>
#include <drm/drm_constraints_owner.h>
#include <drm/drm_file.h>
#include <drm/drm_ioctl.h>
#include <drm/drm_kunit_helpers.h>
#include <kunit/test.h>

#include "../drm_internal.h"

static const struct file_operations test_fops = {
	.owner = THIS_MODULE,
	.release = drm_release_noglobal,
};

static const struct drm_driver test_driver = {
	.driver_features = DRIVER_MODESET | DRIVER_ATOMIC,
	.fops = &test_fops,
};

static void flush_recovery(void *data)
{
	drm_constraints_owner_flush(data);
}

static void close_file(void *data)
{
	__fput_sync(data);
}

static struct drm_device *new_device(struct kunit *test, bool constraints)
{
	struct device *parent = drm_kunit_helper_alloc_device(test);
	struct drm_device *dev;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	dev = __drm_kunit_helper_alloc_drm_device_with_driver(test, parent,
							   sizeof(*dev), 0, &test_driver);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, dev);
	if (constraints)
		KUNIT_ASSERT_EQ(test, drm_constraints_device_init(dev, 4), 0);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, flush_recovery, dev), 0);
	return dev;
}

static struct file *new_file(struct kunit *test, struct drm_device *dev)
{
	struct file *file = mock_drm_getfile(dev->primary, O_RDWR);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, file);
	atomic_inc(&dev->open_count);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, close_file, file), 0);
	return file;
}

static void fail_recovery(struct drm_device *dev)
{
	dev->unplugged = true;
	mutex_lock(&dev->master_mutex);
	drm_constraints_owner_lost(dev);
	mutex_unlock(&dev->master_mutex);
	drm_constraints_owner_flush(dev);
	dev->unplugged = false;
}

static void implicit_master_waits_for_successful_recovery(struct kunit *test)
{
	struct drm_device *dev = new_device(test, true);
	struct file *file = new_file(test, dev);
	struct drm_file *priv = file->private_data;

	fail_recovery(dev);
	KUNIT_ASSERT_EQ(test, drm_master_open(priv), -ENODEV);
	KUNIT_EXPECT_PTR_EQ(test, dev->master, NULL);
	KUNIT_EXPECT_PTR_EQ(test, priv->master, NULL);
	KUNIT_EXPECT_FALSE(test, priv->is_master);
	drm_constraints_owner_flush(dev);
	KUNIT_ASSERT_EQ(test, drm_master_open(priv), 0);
	KUNIT_EXPECT_TRUE(test, drm_is_current_master(priv));
}

static void implicit_master_cannot_restart_closed_recovery(struct kunit *test)
{
	struct drm_device *dev = new_device(test, true);
	struct file *file = new_file(test, dev);
	struct drm_file *priv = file->private_data;

	drm_constraints_owner_stop(dev);
	KUNIT_EXPECT_EQ(test, drm_master_open(priv), -ENODEV);
	KUNIT_EXPECT_PTR_EQ(test, dev->master, NULL);
	KUNIT_EXPECT_PTR_EQ(test, priv->master, NULL);
}

static void implicit_master_preserves_nonparticipating_devices(struct kunit *test)
{
	struct drm_device *dev = new_device(test, false);
	struct file *file = new_file(test, dev);
	struct file *associated = new_file(test, dev);
	struct drm_file *priv = associated->private_data;

	KUNIT_ASSERT_EQ(test, drm_master_open(file->private_data), 0);
	KUNIT_ASSERT_EQ(test, drm_master_open(priv), 0);
	KUNIT_EXPECT_FALSE(test, drm_is_current_master(priv));
	KUNIT_EXPECT_PTR_EQ(test, priv->master, dev->master);
}

static struct kunit_case drm_constraints_auth_tests[] = {
	KUNIT_CASE(implicit_master_waits_for_successful_recovery),
	KUNIT_CASE(implicit_master_cannot_restart_closed_recovery),
	KUNIT_CASE(implicit_master_preserves_nonparticipating_devices),
	{}
};

static struct kunit_suite drm_constraints_auth_suite = {
	.name = "drm_constraints_auth",
	.test_cases = drm_constraints_auth_tests,
};

kunit_test_suite(drm_constraints_auth_suite);

MODULE_DESCRIPTION("DRM constraints master admission tests");
MODULE_LICENSE("Dual MIT/GPL");
