// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/module.h>
#include <linux/completion.h>
#include <drm/drm_constraints_device.h>
#include <drm/drm_constraints_entry.h>
#include <drm/drm_constraints_owner.h>
#include <drm/drm_drv.h>
#include <drm/drm_kunit_helpers.h>
#include <drm/drm_managed.h>
#include <kunit/test.h>

struct domain_fixture {
	struct drm_device drm;
};

static struct drm_device *new_device(struct kunit *test, struct device **parent)
{
	struct domain_fixture *fixture;

	*parent = drm_kunit_helper_alloc_device(test);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, *parent);
	fixture = drm_kunit_helper_alloc_drm_device(test, *parent, struct domain_fixture, drm,
						   DRIVER_MODESET | DRIVER_ATOMIC);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, fixture);
	return &fixture->drm;
}

static void device_publishes_one_validated_domain(struct kunit *test)
{
	struct device *parent;
	struct drm_device *dev = new_device(test, &parent);
	struct drm_constraints_domain *domain;

	KUNIT_EXPECT_PTR_EQ(test, drm_constraints_device_domain(dev), NULL);
	KUNIT_EXPECT_EQ(test, drm_constraints_device_init(dev, 0), -EINVAL);
	KUNIT_EXPECT_PTR_EQ(test, drm_constraints_device_domain(dev), NULL);
	KUNIT_ASSERT_EQ(test, drm_constraints_device_init(dev, 8), 0);
	domain = drm_constraints_device_domain(dev);
	KUNIT_ASSERT_NOT_NULL(test, domain);
	KUNIT_EXPECT_EQ(test, drm_constraints_device_init(dev, 8), -EBUSY);
	KUNIT_EXPECT_PTR_EQ(test, drm_constraints_device_domain(dev), domain);
}

static void registered_devices_cannot_change_domains(struct kunit *test)
{
	struct device *parent;
	struct drm_device *dev = new_device(test, &parent);

	dev->registered = true;
	KUNIT_EXPECT_EQ(test, drm_constraints_device_init(dev, 8), -EBUSY);
	dev->registered = false;
	KUNIT_EXPECT_PTR_EQ(test, drm_constraints_device_domain(dev), NULL);
}

static void retained_domain_outlives_device_cleanup(struct kunit *test)
{
	struct device *parent;
	struct drm_device *dev = new_device(test, &parent);
	struct drm_constraints_domain *domain;

	KUNIT_ASSERT_EQ(test, drm_constraints_device_init(dev, 8), 0);
	domain = drm_constraints_domain_get(drm_constraints_device_domain(dev));
	drm_kunit_helper_free_device(test, parent);
	drm_constraints_domain_put(drm_constraints_domain_get(domain));
	drm_constraints_domain_put(domain);
}

static void complete_cleanup(struct drm_device *dev, void *data)
{
	complete(data);
}

static void recovery_work_can_release_the_final_device_reference(struct kunit *test)
{
	static const struct drm_driver driver = {
		.driver_features = DRIVER_MODESET | DRIVER_ATOMIC,
		.name = "constraints-owner-test",
	};
	static const struct drm_mode_config_funcs mode_ops;
	struct completion *cleaned = kunit_kzalloc(test, sizeof(*cleaned), GFP_KERNEL);
	struct domain_fixture *fixture;
	struct device *parent;
	struct drm_device *dev;

	KUNIT_ASSERT_NOT_NULL(test, cleaned);
	init_completion(cleaned);
	parent = drm_kunit_helper_alloc_device(test);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	fixture = devm_drm_dev_alloc(parent, &driver, struct domain_fixture, drm);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, fixture);
	dev = &fixture->drm;
	dev->mode_config.funcs = &mode_ops;
	/* Registered first so completion runs after managed mode-config cleanup. */
	KUNIT_ASSERT_EQ(test, drmm_add_action_or_reset(dev, complete_cleanup, cleaned), 0);
	KUNIT_ASSERT_EQ(test, drmm_mode_config_init(dev), 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_device_init(dev, 2), 0);
	mutex_lock(&dev->master_mutex);
	drm_constraints_owner_lost(dev);
	/* The queued reference keeps dev and its mutex alive after parent removal. */
	drm_kunit_helper_free_device(test, parent);
	mutex_unlock(&dev->master_mutex);
	if (!wait_for_completion_timeout(cleaned, HZ)) {
		KUNIT_FAIL(test, "Recovery did not finish managed device cleanup");
		wait_for_completion(cleaned);
	}
}

static void observe_unload(struct drm_device *dev)
{
	int *status = dev->dev_private;

	mutex_lock(&dev->master_mutex);
	*status = drm_constraints_owner_check(dev);
	drm_constraints_owner_lost(dev);
	mutex_unlock(&dev->master_mutex);
}

static void unregistration_stops_recovery_before_driver_unload(struct kunit *test)
{
	static const struct drm_driver driver = {
		.driver_features = DRIVER_MODESET | DRIVER_ATOMIC,
		.unload = observe_unload,
		.name = "constraints-unregister-test",
	};
	struct device *parent = drm_kunit_helper_alloc_device(test);
	struct drm_device *dev;
	int status = 0;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	dev = __drm_kunit_helper_alloc_drm_device_with_driver(test, parent,
							   sizeof(*dev), 0, &driver);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, dev);
	KUNIT_ASSERT_EQ(test, drm_constraints_device_init(dev, 2), 0);
	dev->dev_private = &status;
	KUNIT_ASSERT_EQ(test, drm_dev_register(dev, 0), 0);
	drm_dev_unregister(dev);
	drm_constraints_owner_flush(dev);
	KUNIT_EXPECT_EQ(test, status, -ENODEV);
	dev->dev_private = NULL;
}

static struct kunit_case drm_constraints_device_tests[] = {
	KUNIT_CASE(unregistration_stops_recovery_before_driver_unload),
	KUNIT_CASE(recovery_work_can_release_the_final_device_reference),
	KUNIT_CASE(device_publishes_one_validated_domain),
	KUNIT_CASE(registered_devices_cannot_change_domains),
	KUNIT_CASE(retained_domain_outlives_device_cleanup),
	{}
};

static struct kunit_suite drm_constraints_device_test_suite = {
	.name = "drm_constraints_device",
	.test_cases = drm_constraints_device_tests,
};

kunit_test_suite(drm_constraints_device_test_suite);

MODULE_DESCRIPTION("DRM constraints device ownership tests");
MODULE_LICENSE("Dual MIT/GPL");
