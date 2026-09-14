// SPDX-License-Identifier: GPL-2.0-only

#include <linux/limits.h>
#include <linux/module.h>
#include <drm/drm_capture_resources.h>
#include <kunit/test.h>

static void destroy_resources(void *data)
{
	drm_capture_resources_destroy(data);
}

static struct drm_capture_resources *resources_create(struct kunit *test)
{
	struct drm_capture_resources *resources = drm_capture_resources_create(2);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, resources);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, destroy_resources, resources), 0);
	return resources;
}

static void names_survive_slot_reuse(struct kunit *test)
{
	struct drm_capture_resources *resources = resources_create(test);

	KUNIT_EXPECT_EQ(test, drm_capture_resources_check(resources, 0), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_capture_resources_insert(resources, 7), 0);
	KUNIT_EXPECT_EQ(test, drm_capture_resources_insert(resources, 8), 1);
	KUNIT_EXPECT_EQ(test, drm_capture_resources_insert(resources, 9), -EBUSY);
	KUNIT_EXPECT_EQ(test, drm_capture_resources_remove(resources, 7), 0);
	KUNIT_EXPECT_EQ(test, drm_capture_resources_insert(resources, 7), -ESTALE);
	KUNIT_EXPECT_EQ(test, drm_capture_resources_insert(resources, 9), 0);
	KUNIT_EXPECT_EQ(test, drm_capture_resources_find(resources, 8), 1);
	KUNIT_EXPECT_EQ(test, drm_capture_resources_find(resources, 7), -ENOENT);
	KUNIT_EXPECT_EQ(test, drm_capture_resources_remove(resources, 0), -EINVAL);
}

static void checking_does_not_consume_a_name(struct kunit *test)
{
	struct drm_capture_resources *resources = resources_create(test);

	KUNIT_EXPECT_EQ(test, drm_capture_resources_check(resources, 17), 0);
	KUNIT_EXPECT_EQ(test, drm_capture_resources_check(resources, 17), 0);
	KUNIT_EXPECT_EQ(test, drm_capture_resources_find(resources, 17), -ENOENT);
	KUNIT_EXPECT_EQ(test, drm_capture_resources_insert(resources, 16), 0);
	KUNIT_EXPECT_EQ(test, drm_capture_resources_insert(resources, 17), 1);
}

static void exhaustion_does_not_prevent_removal(struct kunit *test)
{
	struct drm_capture_resources *resources = resources_create(test);

	KUNIT_EXPECT_EQ(test, drm_capture_resources_insert(resources, U64_MAX), 0);
	KUNIT_EXPECT_EQ(test, drm_capture_resources_insert(resources, 1), -EOVERFLOW);
	KUNIT_EXPECT_EQ(test, drm_capture_resources_remove(resources, U64_MAX), 0);
	KUNIT_EXPECT_EQ(test, drm_capture_resources_insert(resources, 1), -EOVERFLOW);
}

static struct kunit_case resource_cases[] = {
	KUNIT_CASE(names_survive_slot_reuse),
	KUNIT_CASE(checking_does_not_consume_a_name),
	KUNIT_CASE(exhaustion_does_not_prevent_removal),
	{}
};

static struct kunit_suite resource_suite = {
	.name = "drm_capture_resources",
	.test_cases = resource_cases,
};
kunit_test_suite(resource_suite);

MODULE_LICENSE("GPL");
