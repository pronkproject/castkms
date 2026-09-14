// SPDX-License-Identifier: GPL-2.0-only

#include <linux/module.h>
#include <drm/drm_capture_authority.h>
#include <drm/drm_capture_creator.h>
#include <kunit/test.h>

struct creator_test {
	struct drm_capture_creator *creator;
	struct drm_capture_authority *authority;
	struct drm_capture_registration *first;
	struct drm_capture_registration *second;
	u32 revokes;
	bool remove_during_revoke;
};

static void revoke(void *data)
{
	struct creator_test *context = data;

	context->revokes++;
	if (context->remove_during_revoke) {
		drm_capture_registration_remove(context->first);
		context->first = NULL;
		drm_capture_registration_remove(context->second);
		context->second = NULL;
	}
}

static void release(void *data)
{
}

static const struct drm_capture_authority_ops ops = {
	.owner = THIS_MODULE,
	.revoke = revoke,
	.release = release,
};

static int creator_init(struct kunit *test)
{
	struct creator_test *context;

	context = kunit_kzalloc(test, sizeof(*context), GFP_KERNEL);
	if (!context)
		return -ENOMEM;
	context->authority = drm_capture_authority_create(&ops, context);
	if (IS_ERR(context->authority))
		return PTR_ERR(context->authority);
	context->creator = drm_capture_creator_create(2);
	if (IS_ERR(context->creator)) {
		drm_capture_authority_put(context->authority);
		return PTR_ERR(context->creator);
	}
	test->priv = context;
	return 0;
}

static void creator_exit(struct kunit *test)
{
	struct creator_test *context = test->priv;

	context->remove_during_revoke = false;
	if (!IS_ERR_OR_NULL(context->first))
		drm_capture_registration_remove(context->first);
	if (!IS_ERR_OR_NULL(context->second))
		drm_capture_registration_remove(context->second);
	if (context->creator)
		drm_capture_creator_close(context->creator);
	drm_capture_authority_put(context->authority);
}

static void register_pair(struct kunit *test)
{
	struct creator_test *context = test->priv;

	context->first = drm_capture_creator_register(context->creator, context->authority);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, context->first);
	context->second = drm_capture_creator_register(context->creator, context->authority);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, context->second);
}

static void close_revokes_with_surviving_registrations(struct kunit *test)
{
	struct creator_test *context = test->priv;

	register_pair(test);
	drm_capture_creator_close(context->creator);
	context->creator = NULL;
	KUNIT_EXPECT_EQ(test, context->revokes, 1);
	KUNIT_EXPECT_TRUE(test, drm_capture_authority_cleanup_done(context->authority));
}

static void removal_does_not_revoke(struct kunit *test)
{
	struct creator_test *context = test->priv;

	register_pair(test);
	drm_capture_registration_remove(context->first);
	context->first = NULL;
	drm_capture_registration_remove(context->second);
	context->second = NULL;
	drm_capture_creator_close(context->creator);
	context->creator = NULL;
	KUNIT_EXPECT_EQ(test, context->revokes, 0);
	KUNIT_EXPECT_FALSE(test, drm_capture_authority_revoked(context->authority));
}

static void limit_counts_only_live_registrations(struct kunit *test)
{
	struct creator_test *context = test->priv;
	struct drm_capture_registration *extra;

	register_pair(test);
	extra = drm_capture_creator_register(context->creator, context->authority);
	if (!IS_ERR(extra))
		drm_capture_registration_remove(extra);
	KUNIT_ASSERT_TRUE(test, IS_ERR(extra));
	KUNIT_EXPECT_EQ(test, PTR_ERR(extra), -EBUSY);
	drm_capture_registration_remove(context->first);
	context->first = drm_capture_creator_register(context->creator, context->authority);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, context->first);
}

static void revoke_can_remove_detached_registrations(struct kunit *test)
{
	struct creator_test *context = test->priv;

	register_pair(test);
	context->remove_during_revoke = true;
	drm_capture_creator_close(context->creator);
	context->creator = NULL;
	KUNIT_EXPECT_EQ(test, context->revokes, 1);
	KUNIT_EXPECT_PTR_EQ(test, context->first, NULL);
	KUNIT_EXPECT_PTR_EQ(test, context->second, NULL);
}

static struct kunit_case creator_cases[] = {
	KUNIT_CASE(close_revokes_with_surviving_registrations),
	KUNIT_CASE(removal_does_not_revoke),
	KUNIT_CASE(limit_counts_only_live_registrations),
	KUNIT_CASE(revoke_can_remove_detached_registrations),
	{}
};

static struct kunit_suite creator_suite = {
	.name = "drm_capture_creator",
	.init = creator_init,
	.exit = creator_exit,
	.test_cases = creator_cases,
};
kunit_test_suite(creator_suite);

MODULE_LICENSE("GPL");
