// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/module.h>
#include <drm/drm_atomic_state_helper.h>
#include <drm/drm_constraints.h>
#include <drm/drm_constraints_entry.h>
#include <drm/drm_crtc.h>
#include <drm/drm_fourcc.h>
#include <kunit/test.h>

static void release_backend(void *data)
{
	unsigned int *released = data;

	(*released)++;
}

static const struct drm_constraints_entry_ops entry_ops = {
	.owner = THIS_MODULE,
	.release = release_backend,
};

static void put_domain(void *data) { drm_constraints_domain_put(data); }
static void put_description(void *data) { drm_constraints_description_put(data); }
static void put_entry(void *data) { drm_constraints_entry_put(data); }

static void put_crtc_state(void *data)
{
	drm_atomic_helper_crtc_destroy_state(NULL, data);
}

static void duplicate_retains_the_exact_backend(struct kunit *test)
{
	const struct drm_constraints_size size = { 64, 32, 64, 32 };
	const struct drm_constraints_format format = {
		.plane_id = 7,
		.format = DRM_FORMAT_XRGB8888,
		.modifier = DRM_FORMAT_MOD_LINEAR,
		.size = size,
	};
	struct drm_constraints_description *description;
	struct drm_constraints_domain *domain;
	struct drm_constraints_entry *entry;
	struct drm_crtc_state *original, *duplicate;
	struct drm_crtc *crtc;
	unsigned int *released;

	released = kunit_kzalloc(test, sizeof(*released), GFP_KERNEL);
	KUNIT_ASSERT_NOT_NULL(test, released);
	domain = drm_constraints_domain_create(1);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, domain);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_domain, domain), 0);
	description = drm_constraints_description_create(&size, &format, 1, NULL, 0);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, description);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_description, description), 0);
	entry = drm_constraints_entry_create(domain, 19, description, &entry_ops, released);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, entry);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_entry, entry), 0);
	crtc = kunit_kzalloc(test, sizeof(*crtc), GFP_KERNEL);
	KUNIT_ASSERT_NOT_NULL(test, crtc);
	original = kzalloc_obj(*original);
	KUNIT_ASSERT_NOT_NULL(test, original);
	original->crtc = crtc;
	original->constraints = drm_constraints_entry_get(entry);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_crtc_state, original), 0);
	crtc->state = original;
	duplicate = drm_atomic_helper_crtc_duplicate_state(crtc);
	KUNIT_ASSERT_NOT_NULL(test, duplicate);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_crtc_state, duplicate), 0);
	KUNIT_EXPECT_PTR_EQ(test, duplicate->constraints, entry);
	kunit_release_action(test, put_entry, entry);
	kunit_release_action(test, put_crtc_state, original);
	crtc->state = duplicate;
	KUNIT_EXPECT_EQ(test, *released, 0);
	KUNIT_EXPECT_PTR_EQ(test, drm_constraints_entry_data(duplicate->constraints), released);
	kunit_release_action(test, put_crtc_state, duplicate);
	crtc->state = NULL;
	KUNIT_EXPECT_EQ(test, *released, 1);
}

static void unconstrained_state_remains_unconstrained(struct kunit *test)
{
	struct drm_crtc *crtc = kunit_kzalloc(test, sizeof(*crtc), GFP_KERNEL);
	struct drm_crtc_state *original, *duplicate;

	KUNIT_ASSERT_NOT_NULL(test, crtc);
	original = kunit_kzalloc(test, sizeof(*original), GFP_KERNEL);
	KUNIT_ASSERT_NOT_NULL(test, original);
	crtc->state = original;
	duplicate = drm_atomic_helper_crtc_duplicate_state(crtc);
	KUNIT_ASSERT_NOT_NULL(test, duplicate);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_crtc_state, duplicate), 0);
	KUNIT_EXPECT_PTR_EQ(test, duplicate->constraints, NULL);
}

static struct kunit_case drm_atomic_constraints_tests[] = {
	KUNIT_CASE(duplicate_retains_the_exact_backend),
	KUNIT_CASE(unconstrained_state_remains_unconstrained),
	{}
};

static struct kunit_suite drm_atomic_constraints_test_suite = {
	.name = "drm_atomic_constraints",
	.test_cases = drm_atomic_constraints_tests,
};

kunit_test_suite(drm_atomic_constraints_test_suite);

MODULE_DESCRIPTION("DRM atomic constraints state ownership tests");
MODULE_LICENSE("Dual MIT/GPL");
