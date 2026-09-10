// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/err.h>
#include <linux/module.h>
#include <drm/drm_atomic_prepare.h>
#include <drm/drm_atomic_prepare_scope.h>
#include <drm/drm_atomic_prepare_ticket.h>
#include <kunit/test.h>

struct scoped_fixture {
	struct drm_prepare_domain *domain;
	struct drm_prepare_scope_entry entries[2];
	struct drm_prepare_source *replacement;
	struct drm_prepare_ticket *ticket;
	struct drm_prepare_attempt *attempt;
	struct drm_prepare_retirement_guard *guard;
	unsigned int installed;
};

static void free_fixture(void *data)
{
	struct scoped_fixture *f = data;
	unsigned int i;

	if (f->attempt)
		drm_prepare_attempt_destroy(f->attempt);
	if (f->guard)
		drm_prepare_retirement_guard_destroy(f->guard);
	if (f->ticket)
		drm_prepare_ticket_put(f->ticket);
	if (f->replacement)
		drm_prepare_source_put(f->replacement);
	for (i = 0; i < ARRAY_SIZE(f->entries); i++) {
		if (f->entries[i].source)
			drm_prepare_source_put(f->entries[i].source);
	}
	drm_prepare_domain_put(f->domain);
}

static struct scoped_fixture *new_fixture(struct kunit *test)
{
	struct scoped_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct drm_prepare_source *source;
	unsigned int i;

	KUNIT_ASSERT_NOT_NULL(test, f);
	f->domain = drm_prepare_domain_create();
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->domain);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, free_fixture, f), 0);
	for (i = 0; i < ARRAY_SIZE(f->entries); i++) {
		source = drm_prepare_source_create_in(f->domain, 2);
		KUNIT_ASSERT_NOT_ERR_OR_NULL(test, source);
		f->entries[i].crtc_id = i + 1;
		f->entries[i].source = source;
	}
	source = drm_prepare_source_create_in(f->domain, 2);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, source);
	f->replacement = source;
	return f;
}

static void prepare(struct kunit *test, struct scoped_fixture *f)
{
	struct drm_prepare_ticket *ticket;
	struct drm_prepare_attempt *attempt;

	ticket = drm_prepare_ticket_create_scoped(f->entries, ARRAY_SIZE(f->entries));
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, ticket);
	f->ticket = ticket;
	attempt = drm_prepare_ticket_reserve(ticket);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, attempt);
	f->attempt = attempt;
}

static int install(void *data)
{
	struct scoped_fixture *f = data;

	f->installed++;
	return 0;
}

static int deny_install(void *data)
{
	return -EACCES;
}

static void mismatched_scope_cannot_install_or_consume(struct kunit *test)
{
	struct scoped_fixture *f = new_fixture(test);
	struct drm_prepare_scope_entry observed[] = { f->entries[1], f->entries[0] };
	struct drm_prepare_read_claim *read;

	prepare(test, f);
	KUNIT_EXPECT_EQ(test, drm_prepare_attempt_commit(f->attempt, install, f, &f->guard),
			-EINVAL);
	KUNIT_EXPECT_EQ(test, drm_prepare_attempt_commit_scoped(f->attempt, observed, 1,
							      install, f, &f->guard), -ESTALE);
	observed[0].source = f->replacement;
	KUNIT_EXPECT_EQ(test, drm_prepare_attempt_commit_scoped(f->attempt, observed, 2,
							      install, f, &f->guard), -ESTALE);
	KUNIT_EXPECT_EQ(test, f->installed, 0);
	KUNIT_EXPECT_PTR_EQ(test, f->guard, NULL);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_status(f->ticket), DRM_PREPARE_TICKET_READY);
	observed[0] = f->entries[1];
	KUNIT_EXPECT_EQ(test, drm_prepare_attempt_commit_scoped(f->attempt, observed, 2,
							      deny_install, f, &f->guard), -EACCES);
	KUNIT_EXPECT_PTR_EQ(test, f->guard, NULL);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_status(f->ticket), DRM_PREPARE_TICKET_READY);
	KUNIT_ASSERT_EQ(test, drm_prepare_attempt_commit_scoped(f->attempt, observed, 2,
							      install, f, &f->guard), 0);
	KUNIT_EXPECT_EQ(test, f->installed, 1);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_status(f->ticket), DRM_PREPARE_TICKET_CONSUMED);
	drm_prepare_ticket_cancel(f->ticket);
	KUNIT_EXPECT_EQ(test, drm_prepare_attempt_commit_scoped(f->attempt, observed, 2,
							      install, f, &f->guard), -EALREADY);
	KUNIT_EXPECT_EQ(test, f->installed, 1);
	drm_prepare_ticket_put(f->ticket);
	f->ticket = NULL;
	drm_prepare_attempt_destroy(f->attempt);
	f->attempt = NULL;
	read = drm_prepare_source_claim(f->entries[0].source);
	KUNIT_EXPECT_EQ(test, PTR_ERR(read), -EBUSY);
	if (!IS_ERR(read))
		drm_prepare_read_release(read, NULL);
	drm_prepare_retirement_guard_destroy(f->guard);
	f->guard = NULL;
	read = drm_prepare_source_claim(f->entries[0].source);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, read);
	drm_prepare_read_release(read, NULL);
}

static void canceled_scope_cannot_install(struct kunit *test)
{
	struct scoped_fixture *f = new_fixture(test);
	struct drm_prepare_read_claim *read;

	prepare(test, f);
	drm_prepare_ticket_cancel(f->ticket);
	KUNIT_EXPECT_EQ(test, drm_prepare_attempt_commit_scoped(f->attempt, f->entries, 2,
							      install, f, &f->guard), -ECANCELED);
	KUNIT_EXPECT_EQ(test, f->installed, 0);
	KUNIT_EXPECT_PTR_EQ(test, f->guard, NULL);
	drm_prepare_attempt_destroy(f->attempt);
	f->attempt = NULL;
	read = drm_prepare_source_claim(f->entries[0].source);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, read);
	drm_prepare_read_release(read, NULL);
}

static void unscoped_ticket_cannot_borrow_an_observed_scope(struct kunit *test)
{
	struct scoped_fixture *f = new_fixture(test);
	struct drm_prepare_retirement_set *set;
	struct drm_prepare_ticket *ticket;
	struct drm_prepare_attempt *attempt;

	set = drm_prepare_retirement_set_create(NULL, 0);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, set);
	ticket = drm_prepare_ticket_create(set);
	drm_prepare_retirement_set_put(set);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, ticket);
	f->ticket = ticket;
	attempt = drm_prepare_ticket_reserve(ticket);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, attempt);
	f->attempt = attempt;
	KUNIT_EXPECT_EQ(test, drm_prepare_attempt_commit_scoped(attempt, f->entries, 2,
							      install, f, &f->guard), -EINVAL);
	KUNIT_EXPECT_EQ(test, f->installed, 0);
	KUNIT_EXPECT_PTR_EQ(test, f->guard, NULL);
	KUNIT_EXPECT_EQ(test, drm_prepare_attempt_commit(attempt, install, f, &f->guard), 0);
	KUNIT_EXPECT_EQ(test, f->installed, 1);
}

static struct kunit_case cases[] = {
	KUNIT_CASE(mismatched_scope_cannot_install_or_consume),
	KUNIT_CASE(canceled_scope_cannot_install),
	KUNIT_CASE(unscoped_ticket_cannot_borrow_an_observed_scope),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_prepare_scoped_ticket",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_LICENSE("GPL");
