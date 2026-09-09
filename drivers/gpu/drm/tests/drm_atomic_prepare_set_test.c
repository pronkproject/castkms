// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/completion.h>
#include <linux/dma-fence.h>
#include <linux/err.h>
#include <linux/kthread.h>
#include <linux/module.h>
#include <drm/drm_atomic_prepare.h>
#include <kunit/test.h>

static void put_domain(void *domain)
{
	drm_prepare_domain_put(domain);
}

static struct drm_prepare_domain *new_domain(struct kunit *test)
{
	struct drm_prepare_domain *domain = drm_prepare_domain_create();

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, domain);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_domain, domain), 0);
	return domain;
}

static void put_source(void *source)
{
	drm_prepare_source_put(source);
}

static struct drm_prepare_source *new_source(struct kunit *test, struct drm_prepare_domain *domain)
{
	struct drm_prepare_source *source = drm_prepare_source_create_in(domain, 2);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, source);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_source, source), 0);
	return source;
}

static void put_set(void *set)
{
	drm_prepare_retirement_set_put(set);
}

static struct drm_prepare_retirement_set *
new_set(struct kunit *test, struct drm_prepare_source **sources, unsigned int count)
{
	struct drm_prepare_retirement_set *set = drm_prepare_retirement_set_create(sources, count);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, set);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_set, set), 0);
	return set;
}

static void expect_admission(struct kunit *test, struct drm_prepare_source *source)
{
	struct drm_prepare_read_claim *read = drm_prepare_source_claim(source);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, read);
	drm_prepare_read_release(read, NULL);
}

static void abandon_read(void *read)
{
	drm_prepare_read_abandon(read);
}

static struct drm_prepare_read_claim *claim_read(struct kunit *test,
					       struct drm_prepare_source *source)
{
	struct drm_prepare_read_claim *read = drm_prepare_source_claim(source);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, read);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, abandon_read, read), 0);
	return read;
}

static void release_read(struct kunit *test, struct drm_prepare_read_claim *read)
{
	kunit_remove_action(test, abandon_read, read);
	drm_prepare_read_release(read, NULL);
}

static void readiness_requires_every_member(struct kunit *test)
{
	struct drm_prepare_domain *domain = new_domain(test);
	struct drm_prepare_source *sources[] = { new_source(test, domain), new_source(test, domain) };
	struct drm_prepare_read_claim *a = claim_read(test, sources[0]);
	struct drm_prepare_read_claim *b = claim_read(test, sources[1]);
	struct drm_prepare_retirement_set *set = new_set(test, sources, 2);
	struct dma_fence *fence = ERR_PTR(-EINVAL);

	KUNIT_EXPECT_EQ(test, drm_prepare_retirement_set_ready(set), -EAGAIN);
	KUNIT_EXPECT_EQ(test, drm_prepare_retirement_set_completion(set, &fence), -EAGAIN);
	KUNIT_EXPECT_PTR_EQ(test, fence, ERR_PTR(-EINVAL));
	release_read(test, a);
	KUNIT_EXPECT_EQ(test, drm_prepare_retirement_set_ready(set), -EAGAIN);
	release_read(test, b);
	KUNIT_EXPECT_EQ(test, drm_prepare_retirement_set_ready(set), 0);
	KUNIT_ASSERT_EQ(test, drm_prepare_retirement_set_completion(set, &fence), 0);
	KUNIT_EXPECT_PTR_EQ(test, fence, NULL);
}

static void terminal_failure_takes_precedence_over_pending(struct kunit *test)
{
	struct drm_prepare_domain *domain = new_domain(test);
	struct drm_prepare_source *sources[] = { new_source(test, domain), new_source(test, domain) };
	struct drm_prepare_read_claim *pending, *failed;
	struct drm_prepare_retirement_set *set;
	struct dma_fence *fence = ERR_PTR(-EINVAL);

	/* Creation sorts by address: put the pending member first in that order. */
	if ((unsigned long)sources[0] > (unsigned long)sources[1])
		swap(sources[0], sources[1]);
	pending = claim_read(test, sources[0]);
	failed = claim_read(test, sources[1]);
	set = new_set(test, sources, 2);
	kunit_release_action(test, abandon_read, failed);
	KUNIT_EXPECT_EQ(test, drm_prepare_retirement_set_ready(set), -EIO);
	KUNIT_EXPECT_EQ(test, drm_prepare_retirement_set_completion(set, &fence), -EIO);
	KUNIT_EXPECT_PTR_EQ(test, fence, ERR_PTR(-EINVAL));
	release_read(test, pending);
	KUNIT_EXPECT_EQ(test, drm_prepare_retirement_set_ready(set), -EIO);
}

static void empty_and_invalid_sets(struct kunit *test)
{
	struct drm_prepare_domain *domain = new_domain(test);
	struct drm_prepare_source *source = new_source(test, domain);
	struct drm_prepare_source *invalid[] = { source, NULL };

	new_set(test, NULL, 0);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_retirement_set_create(NULL, 1)), -EINVAL);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_retirement_set_create(invalid, 2)), -EINVAL);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_source_create_in(NULL, 1)), -EINVAL);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_source_create_in(domain, 0)), -EINVAL);
	expect_admission(test, source);
}

static void duplicate_members_and_retained_owner(struct kunit *test)
{
	struct drm_prepare_domain *domain = new_domain(test);
	struct drm_prepare_source *a = new_source(test, domain);
	struct drm_prepare_source *b = new_source(test, domain);
	struct drm_prepare_source *sources[] = { a, b, a, b };
	struct drm_prepare_retirement_set *set = new_set(test, sources, ARRAY_SIZE(sources));

	KUNIT_EXPECT_PTR_EQ(test, sources[0], a);
	KUNIT_EXPECT_PTR_EQ(test, sources[1], b);
	drm_prepare_retirement_set_get(set);
	kunit_release_action(test, put_set, set);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_source_claim(a)), -EBUSY);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_source_claim(b)), -EBUSY);
	drm_prepare_retirement_set_put(set);
	expect_admission(test, a);
	expect_admission(test, b);
}

static void overlapping_sets_release_only_their_holds(struct kunit *test)
{
	struct drm_prepare_domain *domain = new_domain(test);
	struct drm_prepare_source *a = new_source(test, domain);
	struct drm_prepare_source *b = new_source(test, domain);
	struct drm_prepare_source *c = new_source(test, domain);
	struct drm_prepare_source *first[] = { a, b }, *second[] = { c, b };
	struct drm_prepare_retirement_set *one = new_set(test, first, 2);
	struct drm_prepare_retirement_set *two = new_set(test, second, 2);

	kunit_release_action(test, put_set, one);
	expect_admission(test, a);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_source_claim(b)), -EBUSY);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_source_claim(c)), -EBUSY);
	kunit_release_action(test, put_set, two);
	expect_admission(test, b);
	expect_admission(test, c);
}

static void failed_member_leaves_no_partial_holds(struct kunit *test)
{
	struct drm_prepare_domain *domain = new_domain(test);
	struct drm_prepare_source *a = new_source(test, domain);
	struct drm_prepare_source *b = new_source(test, domain);
	struct drm_prepare_source *sources[] = { a, b };
	struct drm_prepare_read_claim *read;
	unsigned int i;

	/* Fail the last member in the acquisition order after validating a live one. */
	if ((unsigned long)a > (unsigned long)b) {
		sources[0] = b;
		sources[1] = a;
	}
	read = drm_prepare_source_claim(sources[1]);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, read);
	drm_prepare_read_abandon(read);
	for (i = 0; i < 3; i++) {
		KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_retirement_set_create(sources, 2)), -EIO);
		expect_admission(test, sources[0]);
	}
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_source_claim(sources[1])), -EIO);
}

static void domains_are_independent(struct kunit *test)
{
	struct drm_prepare_domain *first = new_domain(test), *second = new_domain(test);
	struct drm_prepare_source *a = new_source(test, first);
	struct drm_prepare_source *b = new_source(test, second);
	struct drm_prepare_source *sources[] = { a, b };
	struct drm_prepare_retirement_set *set;

	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_retirement_set_create(sources, 2)), -EXDEV);
	expect_admission(test, a);
	expect_admission(test, b);
	set = new_set(test, sources, 1);
	expect_admission(test, b);
	kunit_release_action(test, put_set, set);
	expect_admission(test, a);
}

static void members_retain_domain_and_source_lifetime(struct kunit *test)
{
	struct drm_prepare_domain *domain = new_domain(test);
	struct drm_prepare_source *source = new_source(test, domain);
	struct drm_prepare_retirement_set *set = new_set(test, &source, 1);

	kunit_release_action(test, put_domain, domain);
	kunit_release_action(test, put_source, source);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_source_claim(source)), -EBUSY);
	kunit_release_action(test, put_set, set);
}

static void release_does_not_resolve_existing_claims(struct kunit *test)
{
	struct drm_prepare_domain *domain = new_domain(test);
	struct drm_prepare_source *source = new_source(test, domain);
	struct drm_prepare_read_claim *read = drm_prepare_source_claim(source);
	struct drm_prepare_retirement_set *set;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, read);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, abandon_read, read), 0);
	set = drm_prepare_retirement_set_create(&source, 1);
	if (IS_ERR(set)) {
		KUNIT_FAIL(test, "retirement set allocation failed");
		return;
	}
	drm_prepare_retirement_set_put(set);
	expect_admission(test, source);
	drm_prepare_source_seal(source);
	KUNIT_EXPECT_EQ(test, drm_prepare_source_ready(source), -EAGAIN);
	kunit_remove_action(test, abandon_read, read);
	drm_prepare_read_release(read, NULL);
	KUNIT_EXPECT_EQ(test, drm_prepare_source_ready(source), 0);
}

struct set_race {
	struct drm_prepare_source *sources[2];
	struct drm_prepare_retirement_set *set;
	struct completion start;
	struct completion done;
};

static int create_overlapping_set(void *data)
{
	struct set_race *race = data;

	wait_for_completion(&race->start);
	race->set = drm_prepare_retirement_set_create(race->sources, 2);
	complete(&race->done);
	while (!kthread_should_stop())
		schedule_timeout_interruptible(1);
	return 0;
}

static void opposite_order_sets_acquire_without_deadlock(struct kunit *test)
{
	struct drm_prepare_domain *domain = new_domain(test);
	struct drm_prepare_source *a = new_source(test, domain);
	struct drm_prepare_source *b = new_source(test, domain);
	struct drm_prepare_source *sources[] = { a, b };
	unsigned int i;

	for (i = 0; i < 16; i++) {
		struct set_race race = { .sources = { b, a } };
		struct drm_prepare_retirement_set *set;
		struct task_struct *worker;
		unsigned long finished;

		init_completion(&race.start);
		init_completion(&race.done);
		worker = kthread_run(create_overlapping_set, &race, "prepare-set");
		KUNIT_ASSERT_NOT_ERR_OR_NULL(test, worker);
		complete(&race.start);
		set = drm_prepare_retirement_set_create(sources, 2);
		finished = wait_for_completion_timeout(&race.done, HZ);
		kthread_stop(worker);
		KUNIT_EXPECT_NE(test, finished, 0);
		KUNIT_EXPECT_FALSE(test, IS_ERR_OR_NULL(set));
		KUNIT_EXPECT_FALSE(test, IS_ERR_OR_NULL(race.set));
		if (!IS_ERR_OR_NULL(set))
			drm_prepare_retirement_set_put(set);
		if (!IS_ERR_OR_NULL(race.set))
			drm_prepare_retirement_set_put(race.set);
		expect_admission(test, a);
		expect_admission(test, b);
	}
}

static struct kunit_case cases[] = {
	KUNIT_CASE(readiness_requires_every_member),
	KUNIT_CASE(terminal_failure_takes_precedence_over_pending),
	KUNIT_CASE(empty_and_invalid_sets),
	KUNIT_CASE(duplicate_members_and_retained_owner),
	KUNIT_CASE(overlapping_sets_release_only_their_holds),
	KUNIT_CASE(failed_member_leaves_no_partial_holds),
	KUNIT_CASE(domains_are_independent),
	KUNIT_CASE(members_retain_domain_and_source_lifetime),
	KUNIT_CASE(release_does_not_resolve_existing_claims),
	KUNIT_CASE(opposite_order_sets_acquire_without_deadlock),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_prepare_set",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_LICENSE("GPL");
