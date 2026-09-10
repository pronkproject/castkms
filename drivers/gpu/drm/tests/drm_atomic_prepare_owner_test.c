// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/err.h>
#include <linux/module.h>
#include <drm/drm_atomic_prepare.h>
#include <drm/drm_atomic_prepare_owner.h>
#include <drm/drm_atomic_prepare_outputs.h>
#include <drm/drm_atomic_prepare_ticket.h>
#include <kunit/test.h>

static void put_owner(void *data)
{
	drm_prepare_owner_revoke(data);
	drm_prepare_owner_put(data);
}

static void put_ticket(void *data)
{
	drm_prepare_ticket_put(data);
}

static void put_source(void *data)
{
	drm_prepare_source_put(data);
}

static void destroy_attempt(void *data)
{
	drm_prepare_attempt_destroy(data);
}

static struct drm_prepare_owner *new_owner(struct kunit *test, unsigned int limit)
{
	struct drm_prepare_owner *owner = drm_prepare_owner_create(limit);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, owner);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_owner, owner), 0);
	return owner;
}

static struct drm_prepare_ticket *new_ticket(struct kunit *test, struct drm_prepare_owner *owner,
					    const struct drm_prepare_output_generation *entry)
{
	struct drm_prepare_ticket *ticket = drm_prepare_ticket_create_owned(owner, entry, !!entry);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, ticket);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_ticket, ticket), 0);
	return ticket;
}

static int install(void *data)
{
	unsigned int *installed = data;

	(*installed)++;
	return 0;
}

static void issuer_identity_cannot_be_substituted(struct kunit *test)
{
	struct drm_prepare_owner *owner = new_owner(test, 1);
	struct drm_prepare_owner *other = new_owner(test, 1);
	struct drm_prepare_ticket *ticket = new_ticket(test, owner, NULL);
	struct drm_prepare_retirement_guard *guard;
	struct drm_prepare_attempt *attempt;
	unsigned int installed = 0;
	int ret;

	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_ticket_reserve(ticket)), -EACCES);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_ticket_reserve_owned(ticket, other)), -EACCES);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_ticket_reserve_owned(ticket, NULL)), -EINVAL);
	attempt = drm_prepare_ticket_reserve_owned(ticket, owner);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, attempt);
	ret = drm_prepare_attempt_commit(attempt, NULL, 0, install, &installed, &guard);
	drm_prepare_attempt_destroy(attempt);
	KUNIT_ASSERT_EQ(test, ret, 0);
	drm_prepare_owner_revoke(owner);
	KUNIT_EXPECT_EQ(test, installed, 1);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_status(ticket), DRM_PREPARE_TICKET_CONSUMED);
	drm_prepare_retirement_guard_destroy(guard);
}

static void revocation_prevents_reserved_acceptance(struct kunit *test)
{
	struct drm_prepare_owner *owner = new_owner(test, 2);
	struct drm_prepare_source *source = drm_prepare_source_create(1);
	struct drm_prepare_output_generation entry = { .crtc_id = 1, .source = source };
	struct drm_prepare_ticket *ticket, *pending;
	struct drm_prepare_retirement_guard *guard = NULL;
	struct drm_prepare_attempt *attempt;
	struct drm_prepare_read_claim *read;
	unsigned int installed = 0;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, source);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_source, source), 0);
	ticket = new_ticket(test, owner, &entry);
	pending = new_ticket(test, owner, NULL);
	attempt = drm_prepare_ticket_reserve_owned(ticket, owner);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, attempt);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, destroy_attempt, attempt), 0);
	drm_prepare_owner_revoke(owner);
	drm_prepare_owner_revoke(owner);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_status(ticket), DRM_PREPARE_TICKET_CANCELED);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_status(pending), DRM_PREPARE_TICKET_CANCELED);
	KUNIT_EXPECT_EQ(test, drm_prepare_attempt_commit(attempt, &entry, 1, install,
							      &installed, &guard), -ECANCELED);
	KUNIT_EXPECT_PTR_EQ(test, guard, NULL);
	KUNIT_EXPECT_EQ(test, installed, 0);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_ticket_reserve_owned(pending, owner)), -ECANCELED);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_ticket_create_owned(owner, NULL, 0)), -ECANCELED);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_source_claim(source)), -EBUSY);
	kunit_release_action(test, destroy_attempt, attempt);
	read = drm_prepare_source_claim(source);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, read);
	drm_prepare_read_release(read, NULL);
}

static void ticket_release_restores_bounded_capacity(struct kunit *test)
{
	struct drm_prepare_owner *owner = new_owner(test, 1);
	struct drm_prepare_ticket *ticket = new_ticket(test, owner, NULL);

	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_owner_create(0)), -EINVAL);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_ticket_create_owned(NULL, NULL, 0)), -EINVAL);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_ticket_create_owned(owner, NULL, 0)), -ENOSPC);
	drm_prepare_ticket_cancel(ticket);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_ticket_create_owned(owner, NULL, 0)), -ENOSPC);
	kunit_release_action(test, put_ticket, ticket);
	ticket = new_ticket(test, owner, NULL);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_status(ticket), DRM_PREPARE_TICKET_READY);
	/* Ticket references keep identity alive, without extending authority. */
	kunit_release_action(test, put_owner, owner);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_status(ticket), DRM_PREPARE_TICKET_CANCELED);
}

static struct kunit_case cases[] = {
	KUNIT_CASE(issuer_identity_cannot_be_substituted),
	KUNIT_CASE(revocation_prevents_reserved_acceptance),
	KUNIT_CASE(ticket_release_restores_bounded_capacity),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_prepare_owner",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_LICENSE("GPL");
