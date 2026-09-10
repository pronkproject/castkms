// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/err.h>
#include <linux/module.h>
#include <drm/drm_atomic_prepare.h>
#include <drm/drm_atomic_prepare_outputs.h>
#include <kunit/test.h>

static void put_domain(void *domain)
{
	drm_prepare_domain_put(domain);
}

static void put_source(void *source)
{
	drm_prepare_source_put(source);
}

static void destroy_outputs(void *outputs)
{
	drm_prepare_outputs_destroy(outputs);
}

static struct drm_prepare_source *new_source(struct kunit *test,
					    struct drm_prepare_domain *domain)
{
	struct drm_prepare_source *source = drm_prepare_source_create_in(domain, 2);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, source);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_source, source), 0);
	return source;
}

static struct drm_prepare_domain *new_domain(struct kunit *test)
{
	struct drm_prepare_domain *domain = drm_prepare_domain_create();

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, domain);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_domain, domain), 0);
	return domain;
}

static struct drm_prepare_outputs *new_outputs(struct kunit *test,
					   const struct drm_prepare_output_generation *entries,
					   unsigned int count)
{
	struct drm_prepare_outputs *outputs = drm_prepare_outputs_create(entries, count);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, outputs);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, destroy_outputs, outputs), 0);
	return outputs;
}

static void cohort_matches_by_output_and_generation(struct kunit *test)
{
	struct drm_prepare_domain *domain = new_domain(test);
	struct drm_prepare_output_generation entries[] = {
		{ 11, new_source(test, domain) },
		{ 42, new_source(test, domain) },
	};
	struct drm_prepare_outputs *outputs = new_outputs(test, entries, ARRAY_SIZE(entries));
	struct drm_prepare_output_generation observed[] = { entries[1], entries[0] };

	entries[0].crtc_id = 99;
	entries[0].source = NULL;
	KUNIT_EXPECT_EQ(test, drm_prepare_outputs_validate(outputs, observed, 2), 0);
	KUNIT_EXPECT_EQ(test, drm_prepare_outputs_validate(outputs, observed, 1), -ESTALE);
	KUNIT_EXPECT_EQ(test, drm_prepare_outputs_validate(outputs, NULL, 0), -ESTALE);
	observed[0].crtc_id = 43;
	KUNIT_EXPECT_EQ(test, drm_prepare_outputs_validate(outputs, observed, 2), -ESTALE);
	observed[0] = entries[1];
	observed[0].source = new_source(test, domain);
	KUNIT_EXPECT_EQ(test, drm_prepare_outputs_validate(outputs, observed, 2), -ESTALE);
	observed[0] = entries[1];
	KUNIT_EXPECT_EQ(test, drm_prepare_outputs_validate(outputs, observed, 2), 0);
}

static void malformed_cohorts_are_rejected(struct kunit *test)
{
	struct drm_prepare_domain *domain = new_domain(test);
	struct drm_prepare_output_generation entries[] = {
		{ 11, new_source(test, domain) },
		{ 11, new_source(test, domain) },
	};
	struct drm_prepare_outputs *empty = new_outputs(test, NULL, 0);

	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_outputs_create(entries, 2)), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_prepare_outputs_validate(empty, entries, 2), -EINVAL);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_outputs_create(NULL, 1)), -EINVAL);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_outputs_create(entries, 33)), -E2BIG);
	KUNIT_EXPECT_EQ(test, drm_prepare_outputs_validate(empty, entries, 33), -E2BIG);
	entries[0].crtc_id = 0;
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_outputs_create(entries, 1)), -EINVAL);
	entries[0].crtc_id = 11;
	entries[0].source = NULL;
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_outputs_create(entries, 1)), -EINVAL);
	entries[0].source = entries[1].source;
	entries[1].crtc_id = 42;
	entries[1].source = new_source(test, new_domain(test));
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_outputs_create(entries, 2)), -EXDEV);
	KUNIT_EXPECT_EQ(test, drm_prepare_outputs_validate(empty, NULL, 0), 0);
}

static void scope_retains_identity_without_holding_admission(struct kunit *test)
{
	struct drm_prepare_domain *domain = new_domain(test);
	struct drm_prepare_output_generation entry = { 11, new_source(test, domain) };
	struct drm_prepare_outputs *outputs = new_outputs(test, &entry, 1);
	struct drm_prepare_retirement_set *set;
	struct drm_prepare_read_claim *read;

	kunit_release_action(test, put_source, entry.source);
	read = drm_prepare_source_claim(entry.source);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, read);
	drm_prepare_read_release(read, NULL);
	set = drm_prepare_outputs_hold(outputs);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, set);
	/* Retain a probe reference independently of both owners. */
	drm_prepare_source_get(entry.source);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_source, entry.source), 0);
	kunit_release_action(test, destroy_outputs, outputs);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_source_claim(entry.source)), -EBUSY);
	drm_prepare_retirement_set_put(set);
	read = drm_prepare_source_claim(entry.source);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, read);
	drm_prepare_read_release(read, NULL);
	kunit_release_action(test, put_source, entry.source);
}

static struct kunit_case cases[] = {
	KUNIT_CASE(cohort_matches_by_output_and_generation),
	KUNIT_CASE(malformed_cohorts_are_rejected),
	KUNIT_CASE(scope_retains_identity_without_holding_admission),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_prepare_outputs",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_LICENSE("GPL");
