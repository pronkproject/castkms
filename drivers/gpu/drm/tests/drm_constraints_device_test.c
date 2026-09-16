// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/module.h>
#include <drm/drm_constraints_device.h>
#include <drm/drm_constraints_entry.h>
#include <drm/drm_kunit_helpers.h>
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

static struct kunit_case drm_constraints_device_tests[] = {
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
