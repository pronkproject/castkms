// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/err.h>
#include <linux/module.h>
#include <drm/drm_atomic_prepare.h>
#include <kunit/test.h>

static void put_source(void *source)
{
	drm_prepare_source_put(source);
}

static void put_set(void *set)
{
	drm_prepare_retirement_set_put(set);
}

static void destroy_guard(void *guard)
{
	drm_prepare_retirement_guard_destroy(guard);
}

static void abandon_read(void *read)
{
	drm_prepare_read_abandon(read);
}

static struct drm_prepare_source *new_source(struct kunit *test)
{
	struct drm_prepare_source *source = drm_prepare_source_create(1);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, source);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_source, source), 0);
	return source;
}

static struct drm_prepare_retirement_set *new_set(struct kunit *test,
						struct drm_prepare_source *source)
{
	struct drm_prepare_retirement_set *set = drm_prepare_retirement_set_create(&source, 1);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, set);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_set, set), 0);
	return set;
}

static struct drm_prepare_retirement_guard *new_guard(struct kunit *test,
						    struct drm_prepare_retirement_set *set)
{
	struct drm_prepare_retirement_guard *guard = drm_prepare_retirement_guard_create(set);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, guard);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, destroy_guard, guard), 0);
	return guard;
}

static struct drm_prepare_read_claim *claim_read(struct kunit *test,
					       struct drm_prepare_source *source)
{
	struct drm_prepare_read_claim *read = drm_prepare_source_claim(source);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, read);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, abandon_read, read), 0);
	return read;
}

static void pending_creation_preserves_the_set_for_retry(struct kunit *test)
{
	struct drm_prepare_source *source = new_source(test);
	struct drm_prepare_read_claim *read = claim_read(test, source);
	struct drm_prepare_retirement_set *set = new_set(test, source);
	struct drm_prepare_retirement_guard *guard;

	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_retirement_guard_create(set)), -EAGAIN);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_source_claim(source)), -EBUSY);
	kunit_remove_action(test, abandon_read, read);
	drm_prepare_read_release(read, NULL);
	guard = new_guard(test, set);
	kunit_release_action(test, put_set, set);
	KUNIT_EXPECT_PTR_EQ(test, drm_prepare_retirement_guard_completion(guard), NULL);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_source_claim(source)), -EBUSY);
	kunit_release_action(test, destroy_guard, guard);
	read = claim_read(test, source);
	kunit_remove_action(test, abandon_read, read);
	drm_prepare_read_release(read, NULL);
}

static void abandoned_claim_cannot_create_a_guard(struct kunit *test)
{
	struct drm_prepare_source *source = new_source(test);
	struct drm_prepare_read_claim *read = claim_read(test, source);
	struct drm_prepare_retirement_set *set = new_set(test, source);

	kunit_release_action(test, abandon_read, read);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_retirement_guard_create(set)), -EIO);
	KUNIT_EXPECT_EQ(test, drm_prepare_retirement_set_ready(set), -EIO);
}

static void independent_guards_retain_their_own_admission(struct kunit *test)
{
	struct drm_prepare_source *source = new_source(test);
	struct drm_prepare_retirement_set *set = new_set(test, source);
	struct drm_prepare_retirement_guard *a = new_guard(test, set);
	struct drm_prepare_retirement_guard *b = new_guard(test, set);
	struct drm_prepare_read_claim *read;

	kunit_release_action(test, put_set, set);
	kunit_release_action(test, destroy_guard, a);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_source_claim(source)), -EBUSY);
	kunit_release_action(test, destroy_guard, b);
	read = claim_read(test, source);
	kunit_remove_action(test, abandon_read, read);
	drm_prepare_read_release(read, NULL);
}

static struct kunit_case cases[] = {
	KUNIT_CASE(pending_creation_preserves_the_set_for_retry),
	KUNIT_CASE(abandoned_claim_cannot_create_a_guard),
	KUNIT_CASE(independent_guards_retain_their_own_admission),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_prepare_guard",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_LICENSE("GPL");
