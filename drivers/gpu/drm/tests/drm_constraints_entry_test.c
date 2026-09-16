// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/err.h>
#include <linux/module.h>
#include <drm/drm_constraints.h>
#include <drm/drm_constraints_entry.h>
#include <drm/drm_fourcc.h>
#include <kunit/test.h>

struct entry_fixture {
	struct drm_constraints_domain *domain;
	struct drm_constraints_description *description;
	unsigned int released;
};

static void backend_release(void *data)
{
	struct entry_fixture *fixture = data;

	fixture->released++;
}

static const struct drm_constraints_entry_ops backend_ops = {
	.owner = THIS_MODULE,
	.release = backend_release,
};

static void put_domain(void *data)
{
	drm_constraints_domain_put(data);
}

static void put_description(void *data)
{
	drm_constraints_description_put(data);
}

static void put_entry(void *data)
{
	drm_constraints_entry_put(data);
}

static struct entry_fixture *new_fixture(struct kunit *test, unsigned int limit)
{
	const struct drm_constraints_size size = { 64, 32, 64, 32 };
	const struct drm_constraints_format format = {
		.plane_id = 7,
		.format = DRM_FORMAT_XRGB8888,
		.modifier = DRM_FORMAT_MOD_LINEAR,
		.size = size,
	};
	struct entry_fixture *fixture = kunit_kzalloc(test, sizeof(*fixture), GFP_KERNEL);

	KUNIT_ASSERT_NOT_NULL(test, fixture);
	fixture->domain = drm_constraints_domain_create(limit);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, fixture->domain);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_domain, fixture->domain), 0);
	fixture->description = drm_constraints_description_create(&size, &format, 1);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, fixture->description);
	KUNIT_ASSERT_EQ(test,
		kunit_add_action_or_reset(test, put_description, fixture->description), 0);
	return fixture;
}

static struct drm_constraints_entry *
new_entry(struct kunit *test, struct entry_fixture *fixture, u32 crtc_id)
{
	struct drm_constraints_entry *entry;

	entry = drm_constraints_entry_create(fixture->domain, crtc_id, fixture->description,
					     &backend_ops, fixture);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, entry);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_entry, entry), 0);
	return entry;
}

static void entries_retain_provider_and_description(struct kunit *test)
{
	struct entry_fixture *fixture = new_fixture(test, 2);
	struct drm_constraints_entry *entry = new_entry(test, fixture, 19);

	KUNIT_EXPECT_PTR_EQ(test, drm_constraints_entry_data(entry), fixture);
	KUNIT_EXPECT_EQ(test, drm_constraints_entry_crtc(entry), 19);
	KUNIT_EXPECT_PTR_EQ(test, drm_constraints_entry_description(entry), fixture->description);
	kunit_release_action(test, put_description, fixture->description);
	kunit_release_action(test, put_domain, fixture->domain);
	KUNIT_EXPECT_EQ(test,
		drm_constraints_description_output(drm_constraints_entry_description(entry))->min_width,
		64);
	KUNIT_EXPECT_PTR_EQ(test, drm_constraints_entry_get(entry), entry);
	drm_constraints_entry_put(entry);
	KUNIT_EXPECT_EQ(test, fixture->released, 0);
	kunit_release_action(test, put_entry, entry);
	KUNIT_EXPECT_EQ(test, fixture->released, 1);
}

static void identities_are_not_reused_across_outputs(struct kunit *test)
{
	struct entry_fixture *fixture = new_fixture(test, 1);
	struct drm_constraints_entry *entry = new_entry(test, fixture, 19);
	u64 first = drm_constraints_entry_id(entry);

	KUNIT_EXPECT_GT(test, first, 0);
	kunit_release_action(test, put_entry, entry);
	entry = new_entry(test, fixture, 23);
	KUNIT_EXPECT_GT(test, drm_constraints_entry_id(entry), first);
	KUNIT_EXPECT_EQ(test, drm_constraints_entry_crtc(entry), 23);
}

static void retained_entries_keep_their_quota(struct kunit *test)
{
	struct entry_fixture *fixture = new_fixture(test, 1);
	struct drm_constraints_entry *entry = new_entry(test, fixture, 19);

	drm_constraints_entry_get(entry);
	kunit_release_action(test, put_entry, entry);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_entry, entry), 0);
	KUNIT_EXPECT_PTR_EQ(test,
		drm_constraints_entry_create(fixture->domain, 19, fixture->description,
					     &backend_ops, fixture), ERR_PTR(-ENOSPC));
	KUNIT_EXPECT_EQ(test, fixture->released, 0);
	kunit_release_action(test, put_entry, entry);
	KUNIT_EXPECT_EQ(test, fixture->released, 1);
	entry = new_entry(test, fixture, 19);
	KUNIT_EXPECT_EQ(test, drm_constraints_entry_id(entry), 2);
}

static void identities_remain_scoped_to_their_domain(struct kunit *test)
{
	struct entry_fixture *first = new_fixture(test, 1);
	struct entry_fixture *second = new_fixture(test, 1);
	struct drm_constraints_entry *entry = new_entry(test, first, 19);

	KUNIT_EXPECT_TRUE(test, drm_constraints_entry_in_domain(entry, first->domain));
	KUNIT_EXPECT_FALSE(test, drm_constraints_entry_in_domain(entry, second->domain));
}

static void rejected_creation_leaves_provider_owned_by_caller(struct kunit *test)
{
	struct entry_fixture *fixture = new_fixture(test, 1);

	KUNIT_EXPECT_PTR_EQ(test, drm_constraints_domain_create(0), ERR_PTR(-EINVAL));
	KUNIT_EXPECT_PTR_EQ(test,
		drm_constraints_entry_create(fixture->domain, 0, fixture->description,
					     &backend_ops, fixture), ERR_PTR(-EINVAL));
	KUNIT_EXPECT_PTR_EQ(test,
		drm_constraints_entry_create(fixture->domain, 19, fixture->description,
					     NULL, fixture), ERR_PTR(-EINVAL));
	KUNIT_EXPECT_EQ(test, fixture->released, 0);
	KUNIT_EXPECT_EQ(test, drm_constraints_entry_id(new_entry(test, fixture, 19)), 1);
}

static struct kunit_case drm_constraints_entry_tests[] = {
	KUNIT_CASE(entries_retain_provider_and_description),
	KUNIT_CASE(identities_are_not_reused_across_outputs),
	KUNIT_CASE(retained_entries_keep_their_quota),
	KUNIT_CASE(identities_remain_scoped_to_their_domain),
	KUNIT_CASE(rejected_creation_leaves_provider_owned_by_caller),
	{}
};

static struct kunit_suite drm_constraints_entry_test_suite = {
	.name = "drm_constraints_entry",
	.test_cases = drm_constraints_entry_tests,
};

kunit_test_suite(drm_constraints_entry_test_suite);

MODULE_LICENSE("Dual MIT/GPL");
