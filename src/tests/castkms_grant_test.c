// SPDX-License-Identifier: GPL-2.0-only

#include <kunit/test.h>

#include "../castkms_capture_owner.h"
#include "../castkms_framebuffer.h"

MODULE_IMPORT_NS("EXPORTED_FOR_KUNIT_TESTING");

static void castkms_capture_owner_requires_current_master(struct kunit *test)
{
	static const int master_a;
	static const int master_b;
	const struct drm_master *owner_a = (const void *)&master_a;
	const struct drm_master *owner_b = (const void *)&master_b;

	KUNIT_EXPECT_TRUE(test,
		castkms_capture_owner_is_current(owner_a, owner_a));
	KUNIT_EXPECT_FALSE(test,
		castkms_capture_owner_is_current(owner_a, owner_b));
	KUNIT_EXPECT_FALSE(test,
		castkms_capture_owner_is_current(owner_a, NULL));
	KUNIT_EXPECT_FALSE(test,
		castkms_capture_owner_is_current(NULL, owner_a));
}

static void castkms_framebuffer_keeps_durable_owner(struct kunit *test)
{
	struct drm_master owner_a = {};
	struct drm_master owner_b = {};

	KUNIT_EXPECT_PTR_EQ(test,
		castkms_framebuffer_resolve_capture_owner(&owner_a, NULL,
							   &owner_b),
		&owner_a);
}

static void castkms_framebuffer_accepts_current_association(struct kunit *test)
{
	struct drm_master owner_a = {};

	KUNIT_EXPECT_PTR_EQ(test,
		castkms_framebuffer_resolve_capture_owner(NULL, &owner_a,
							   &owner_a),
		&owner_a);
}

static void castkms_framebuffer_rejects_stale_association(struct kunit *test)
{
	struct drm_master owner_a = {};
	struct drm_master owner_b = {};

	KUNIT_EXPECT_PTR_EQ(test,
		castkms_framebuffer_resolve_capture_owner(NULL, &owner_a,
							   &owner_b),
		NULL);
	KUNIT_EXPECT_PTR_EQ(test,
		castkms_framebuffer_resolve_capture_owner(NULL, &owner_a, NULL),
		NULL);
	KUNIT_EXPECT_PTR_EQ(test,
		castkms_framebuffer_resolve_capture_owner(NULL, NULL, &owner_a),
		NULL);
}

static void castkms_framebuffer_rejects_mixed_plane_owners(struct kunit *test)
{
	struct drm_master owner_a = {};
	struct drm_master owner_b = {};

	KUNIT_EXPECT_TRUE(test,
		castkms_framebuffer_capture_owners_match(NULL, &owner_a));
	KUNIT_EXPECT_TRUE(test,
		castkms_framebuffer_capture_owners_match(&owner_a, &owner_a));
	KUNIT_EXPECT_FALSE(test,
		castkms_framebuffer_capture_owners_match(&owner_a, &owner_b));
	KUNIT_EXPECT_FALSE(test,
		castkms_framebuffer_capture_owners_match(&owner_a, NULL));
}

static void castkms_blank_noop_does_not_claim_content(struct kunit *test)
{
	KUNIT_EXPECT_FALSE(test,
		castkms_capture_blank_establishes_owner(true, false, false,
							 false, false));
}

static void castkms_disabling_last_plane_claims_safe_blank(struct kunit *test)
{
	KUNIT_EXPECT_TRUE(test,
		castkms_capture_blank_establishes_owner(true, true, false,
							 false, false));
}

static struct kunit_case castkms_grant_test_cases[] = {
	KUNIT_CASE(castkms_capture_owner_requires_current_master),
	KUNIT_CASE(castkms_framebuffer_keeps_durable_owner),
	KUNIT_CASE(castkms_framebuffer_accepts_current_association),
	KUNIT_CASE(castkms_framebuffer_rejects_stale_association),
	KUNIT_CASE(castkms_framebuffer_rejects_mixed_plane_owners),
	KUNIT_CASE(castkms_blank_noop_does_not_claim_content),
	KUNIT_CASE(castkms_disabling_last_plane_claims_safe_blank),
	{}
};

static struct kunit_suite castkms_grant_test_suite = {
	.name = "castkms-grant",
	.test_cases = castkms_grant_test_cases,
};

kunit_test_suite(castkms_grant_test_suite);

MODULE_DESCRIPTION("KUnit tests for CastKMS capture grant ownership");
MODULE_LICENSE("GPL");
