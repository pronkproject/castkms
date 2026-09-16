// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/err.h>
#include <linux/completion.h>
#include <linux/kthread.h>
#include <linux/module.h>
#include <drm/drm_constraints.h>
#include <drm/drm_constraints_catalog.h>
#include <drm/drm_constraints_entry.h>
#include <drm/drm_fourcc.h>
#include <kunit/test.h>

struct catalog_fixture {
	struct drm_constraints_domain *domain;
	struct drm_constraints_description *description;
	struct drm_constraints_entry *initial;
	struct drm_constraints_catalog *catalog;
	unsigned int released;
};

static void release_backend(void *data)
{
	struct catalog_fixture *fixture = data;

	fixture->released++;
}

static const struct drm_constraints_entry_ops ops = {
	.owner = THIS_MODULE,
	.release = release_backend,
};

static void put_domain(void *data) { drm_constraints_domain_put(data); }
static void put_description(void *data) { drm_constraints_description_put(data); }
static void put_entry(void *data) { drm_constraints_entry_put(data); }
static void put_catalog(void *data) { drm_constraints_catalog_put(data); }
static void put_snapshot(void *data) { drm_constraints_snapshot_put(data); }

static struct drm_constraints_entry *
new_entry(struct kunit *test, struct catalog_fixture *fixture, u32 crtc_id)
{
	struct drm_constraints_entry *entry;

	entry = drm_constraints_entry_create(fixture->domain, crtc_id,
					     fixture->description, &ops, fixture);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, entry);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_entry, entry), 0);
	return entry;
}

static struct catalog_fixture *new_fixture(struct kunit *test, unsigned int limit)
{
	const struct drm_constraints_size size = { 64, 32, 64, 32 };
	const struct drm_constraints_format format = {
		.plane_id = 7,
		.format = DRM_FORMAT_XRGB8888,
		.modifier = DRM_FORMAT_MOD_LINEAR,
		.size = size,
	};
	struct catalog_fixture *fixture = kunit_kzalloc(test, sizeof(*fixture), GFP_KERNEL);

	KUNIT_ASSERT_NOT_NULL(test, fixture);
	fixture->domain = drm_constraints_domain_create(DRM_CONSTRAINTS_MAX_ENTRIES);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, fixture->domain);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_domain, fixture->domain), 0);
	fixture->description = drm_constraints_description_create(&size, &format, 1, NULL, 0);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, fixture->description);
	KUNIT_ASSERT_EQ(test,
		kunit_add_action_or_reset(test, put_description, fixture->description), 0);
	fixture->initial = new_entry(test, fixture, 19);
	fixture->catalog = drm_constraints_catalog_create(fixture->domain, fixture->initial, limit);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, fixture->catalog);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_catalog, fixture->catalog), 0);
	return fixture;
}

static struct drm_constraints_snapshot *
snapshot(struct kunit *test, struct drm_constraints_catalog *catalog, u64 generation)
{
	struct drm_constraints_snapshot *snapshot;

	snapshot = drm_constraints_catalog_snapshot(catalog, generation);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, snapshot);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_snapshot, snapshot), 0);
	return snapshot;
}

static void snapshots_retain_immutable_entries(struct kunit *test)
{
	struct catalog_fixture *fixture = new_fixture(test, 2);
	struct drm_constraints_snapshot *before = snapshot(test, fixture->catalog, 0);
	const struct drm_constraints_listing *entries = drm_constraints_snapshot_entries(before);
	u64 id = drm_constraints_entry_id(fixture->initial);

	KUNIT_ASSERT_EQ(test, drm_constraints_catalog_withdraw(fixture->catalog, id), 0);
	KUNIT_EXPECT_TRUE(test, entries[0].selectable);
	KUNIT_EXPECT_EQ(test, drm_constraints_snapshot_info(before)->generation, 1);
	kunit_release_action(test, put_catalog, fixture->catalog);
	kunit_release_action(test, put_entry, fixture->initial);
	KUNIT_EXPECT_EQ(test, fixture->released, 0);
	KUNIT_EXPECT_EQ(test, drm_constraints_entry_id(entries[0].entry), id);
	kunit_release_action(test, put_snapshot, before);
	KUNIT_EXPECT_EQ(test, fixture->released, 1);
}

static void changes_invalidate_expected_generations(struct kunit *test)
{
	struct catalog_fixture *fixture = new_fixture(test, 2);
	struct drm_constraints_entry *target = new_entry(test, fixture, 19);
	struct drm_constraints_snapshot *before = snapshot(test, fixture->catalog, 0);
	u64 generation = drm_constraints_snapshot_info(before)->generation;

	snapshot(test, fixture->catalog, generation);
	KUNIT_ASSERT_EQ(test, drm_constraints_catalog_add(fixture->catalog, target), 0);
	KUNIT_EXPECT_PTR_EQ(test,
		drm_constraints_catalog_snapshot(fixture->catalog, generation), ERR_PTR(-ESTALE));
	KUNIT_EXPECT_EQ(test,
		drm_constraints_snapshot_info(snapshot(test, fixture->catalog, 0))->count, 2);
}

static void suggestions_do_not_select_entries(struct kunit *test)
{
	struct catalog_fixture *fixture = new_fixture(test, 2);
	struct drm_constraints_entry *target = new_entry(test, fixture, 19);
	const struct drm_constraints_snapshot_info *info;
	u64 id = drm_constraints_entry_id(target), generation;

	KUNIT_ASSERT_EQ(test, drm_constraints_catalog_add(fixture->catalog, target), 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_catalog_suggest(fixture->catalog, id), 0);
	info = drm_constraints_snapshot_info(snapshot(test, fixture->catalog, 0));
	KUNIT_EXPECT_EQ(test, info->selected_id, drm_constraints_entry_id(fixture->initial));
	KUNIT_EXPECT_EQ(test, info->suggested_id, id);
	generation = info->generation;
	KUNIT_ASSERT_EQ(test, drm_constraints_catalog_suggest(fixture->catalog, id), 0);
	snapshot(test, fixture->catalog, generation);
	KUNIT_ASSERT_EQ(test, drm_constraints_catalog_withdraw(fixture->catalog, id), 0);
	info = drm_constraints_snapshot_info(snapshot(test, fixture->catalog, 0));
	KUNIT_EXPECT_EQ(test, info->suggested_id, 0);
	KUNIT_EXPECT_EQ(test, info->generation, generation + 1);
	KUNIT_EXPECT_EQ(test, drm_constraints_catalog_suggest(fixture->catalog, id), -ESTALE);
	KUNIT_ASSERT_EQ(test, drm_constraints_catalog_withdraw(fixture->catalog, id), 0);
	snapshot(test, fixture->catalog, info->generation);
}

static void removing_an_entry_preserves_other_identities(struct kunit *test)
{
	struct catalog_fixture *fixture = new_fixture(test, 3);
	struct drm_constraints_entry *first = new_entry(test, fixture, 19);
	struct drm_constraints_entry *second = new_entry(test, fixture, 19);
	struct drm_constraints_snapshot *before, *after;
	u64 first_id = drm_constraints_entry_id(first);

	KUNIT_ASSERT_EQ(test, drm_constraints_catalog_add(fixture->catalog, first), 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_catalog_add(fixture->catalog, second), 0);
	before = snapshot(test, fixture->catalog, 0);
	KUNIT_EXPECT_EQ(test, drm_constraints_catalog_forget(fixture->catalog, first_id), -EBUSY);
	KUNIT_ASSERT_EQ(test, drm_constraints_catalog_withdraw(fixture->catalog, first_id), 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_catalog_forget(fixture->catalog, first_id), 0);
	after = snapshot(test, fixture->catalog, 0);
	KUNIT_EXPECT_EQ(test, drm_constraints_snapshot_info(before)->count, 3);
	KUNIT_EXPECT_EQ(test, drm_constraints_snapshot_info(after)->count, 2);
	KUNIT_EXPECT_PTR_EQ(test, drm_constraints_snapshot_entries(before)[2].entry,
			   drm_constraints_snapshot_entries(after)[1].entry);
	KUNIT_EXPECT_EQ(test, drm_constraints_catalog_forget(fixture->catalog,
					   drm_constraints_entry_id(fixture->initial)), -EBUSY);
}

static void catalogs_reject_foreign_scope_and_overflow(struct kunit *test)
{
	struct catalog_fixture *fixture = new_fixture(test, 1);
	struct catalog_fixture *other = new_fixture(test, 1);
	struct drm_constraints_entry *wrong_output = new_entry(test, fixture, 23);
	struct drm_constraints_entry *target = new_entry(test, fixture, 19);

	KUNIT_EXPECT_EQ(test, drm_constraints_catalog_add(fixture->catalog, other->initial), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_constraints_catalog_add(fixture->catalog, wrong_output), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_constraints_catalog_add(fixture->catalog, target), -ENOSPC);
	KUNIT_EXPECT_EQ(test, drm_constraints_catalog_add(fixture->catalog, fixture->initial), -EEXIST);
	KUNIT_EXPECT_EQ(test, drm_constraints_catalog_withdraw(fixture->catalog, 0), -ENOENT);
	snapshot(test, fixture->catalog, 1);
}

static void construction_requires_bounded_matching_scope(struct kunit *test)
{
	struct catalog_fixture *fixture = new_fixture(test, 1);
	struct catalog_fixture *other = new_fixture(test, 1);

	KUNIT_EXPECT_PTR_EQ(test,
		drm_constraints_catalog_create(fixture->domain, fixture->initial, 0),
		ERR_PTR(-EINVAL));
	KUNIT_EXPECT_PTR_EQ(test,
		drm_constraints_catalog_create(fixture->domain, fixture->initial,
					       DRM_CONSTRAINTS_MAX_ENTRIES + 1), ERR_PTR(-EINVAL));
	KUNIT_EXPECT_PTR_EQ(test,
		drm_constraints_catalog_create(fixture->domain, other->initial, 1),
		ERR_PTR(-EINVAL));
}

struct install_context {
	struct drm_constraints_entry *accepted;
	unsigned int calls;
	int result;
};

static int validate(struct drm_constraints_entry *entry, void *data)
{
	struct install_context *context = data;

	context->calls++;
	return context->result;
}

static int install(struct drm_constraints_entry *entry, void *data)
{
	struct install_context *context = data;

	context->calls++;
	if (context->result)
		return context->result;
	context->accepted = drm_constraints_entry_get(entry);
	return 0;
}

static void test_only_does_not_reserve_selection(struct kunit *test)
{
	struct catalog_fixture *fixture = new_fixture(test, 2);
	struct drm_constraints_entry *target = new_entry(test, fixture, 19);
	struct install_context context = {};
	u64 id = drm_constraints_entry_id(target);

	KUNIT_ASSERT_EQ(test, drm_constraints_catalog_add(fixture->catalog, target), 0);
	KUNIT_ASSERT_EQ(test,
		drm_constraints_catalog_check(fixture->catalog, target, validate, &context), 0);
	KUNIT_EXPECT_EQ(test,
		drm_constraints_snapshot_info(snapshot(test, fixture->catalog, 0))->selected_id,
		drm_constraints_entry_id(fixture->initial));
	KUNIT_ASSERT_EQ(test, drm_constraints_catalog_withdraw(fixture->catalog, id), 0);
	KUNIT_EXPECT_EQ(test,
		drm_constraints_catalog_accept(fixture->catalog, target, install, &context), -ESTALE);
	KUNIT_EXPECT_EQ(test, context.calls, 1);
	KUNIT_EXPECT_PTR_EQ(test, context.accepted, NULL);
}

static void accepted_selection_survives_withdrawal(struct kunit *test)
{
	struct catalog_fixture *fixture = new_fixture(test, 2);
	struct drm_constraints_entry *target = new_entry(test, fixture, 19);
	struct install_context context = {};
	const struct drm_constraints_snapshot_info *info;
	u64 id = drm_constraints_entry_id(target), generation;

	KUNIT_ASSERT_EQ(test, drm_constraints_catalog_add(fixture->catalog, target), 0);
	KUNIT_ASSERT_EQ(test,
		drm_constraints_catalog_accept(fixture->catalog, target, install, &context), 0);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_entry, context.accepted), 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_catalog_withdraw(fixture->catalog, id), 0);
	info = drm_constraints_snapshot_info(snapshot(test, fixture->catalog, 0));
	KUNIT_EXPECT_EQ(test, info->selected_id, id);
	generation = info->generation;
	KUNIT_ASSERT_EQ(test,
		drm_constraints_catalog_accept(fixture->catalog, target, validate, &context), 0);
	snapshot(test, fixture->catalog, generation);
	kunit_release_action(test, put_catalog, fixture->catalog);
	KUNIT_EXPECT_EQ(test, drm_constraints_entry_id(context.accepted), id);
}

static void failed_installation_preserves_selection(struct kunit *test)
{
	struct catalog_fixture *fixture = new_fixture(test, 2);
	struct drm_constraints_entry *target = new_entry(test, fixture, 19);
	struct install_context context = { .result = -ENOMEM };
	const struct drm_constraints_snapshot_info *info;
	u64 generation;

	KUNIT_ASSERT_EQ(test, drm_constraints_catalog_add(fixture->catalog, target), 0);
	info = drm_constraints_snapshot_info(snapshot(test, fixture->catalog, 0));
	generation = info->generation;
	KUNIT_EXPECT_EQ(test,
		drm_constraints_catalog_accept(fixture->catalog, target, install, &context), -ENOMEM);
	info = drm_constraints_snapshot_info(snapshot(test, fixture->catalog, generation));
	KUNIT_EXPECT_EQ(test, info->selected_id, drm_constraints_entry_id(fixture->initial));
	KUNIT_EXPECT_PTR_EQ(test, context.accepted, NULL);
	context.result = 1;
	KUNIT_EXPECT_EQ(test,
		drm_constraints_catalog_accept(fixture->catalog, target, install, &context), -EINVAL);
	snapshot(test, fixture->catalog, generation);
}

static void equal_ids_do_not_authorize_foreign_entries(struct kunit *test)
{
	struct catalog_fixture *fixture = new_fixture(test, 1);
	struct catalog_fixture *other = new_fixture(test, 1);
	struct install_context context = {};

	KUNIT_ASSERT_EQ(test, drm_constraints_entry_id(fixture->initial),
			drm_constraints_entry_id(other->initial));
	KUNIT_EXPECT_EQ(test,
		drm_constraints_catalog_accept(fixture->catalog, other->initial, install, &context),
		-ESTALE);
	KUNIT_EXPECT_EQ(test, context.calls, 0);
}

static void closing_rejects_checked_but_unaccepted_selection(struct kunit *test)
{
	struct catalog_fixture *fixture = new_fixture(test, 1);
	struct install_context context = {};
	struct drm_constraints_snapshot *before = snapshot(test, fixture->catalog, 0);

	KUNIT_ASSERT_EQ(test,
		drm_constraints_catalog_check(fixture->catalog, fixture->initial, validate, &context), 0);
	drm_constraints_catalog_close(fixture->catalog);
	drm_constraints_catalog_close(fixture->catalog);
	KUNIT_EXPECT_EQ(test,
		drm_constraints_catalog_accept(fixture->catalog, fixture->initial, install, &context),
		-ESTALE);
	KUNIT_EXPECT_EQ(test, context.calls, 1);
	KUNIT_EXPECT_PTR_EQ(test, drm_constraints_catalog_snapshot(fixture->catalog, 0),
			   ERR_PTR(-ESTALE));
	KUNIT_EXPECT_EQ(test, drm_constraints_catalog_add(fixture->catalog, fixture->initial), -ESTALE);
	KUNIT_EXPECT_EQ(test, drm_constraints_catalog_suggest(fixture->catalog, 0), -ESTALE);
	KUNIT_EXPECT_EQ(test, drm_constraints_snapshot_info(before)->selected_id,
			drm_constraints_entry_id(fixture->initial));
}

struct close_race {
	struct drm_constraints_catalog *catalog;
	struct drm_constraints_entry *entry;
	struct completion installing;
	struct completion finish;
	struct completion closing;
	struct completion closed;
	int result;
};

static int paused_install(struct drm_constraints_entry *entry, void *data)
{
	struct close_race *race = data;

	complete(&race->installing);
	return wait_for_completion_timeout(&race->finish, HZ) ? 0 : -ETIMEDOUT;
}

static int accept_worker(void *data)
{
	struct close_race *race = data;

	race->result = drm_constraints_catalog_accept(race->catalog, race->entry,
						      paused_install, race);
	while (!kthread_should_stop())
		schedule_timeout_interruptible(1);
	return 0;
}

static int close_worker(void *data)
{
	struct close_race *race = data;

	complete(&race->closing);
	drm_constraints_catalog_close(race->catalog);
	complete(&race->closed);
	while (!kthread_should_stop())
		schedule_timeout_interruptible(1);
	return 0;
}

static void closing_waits_for_irrevocable_acceptance(struct kunit *test)
{
	struct catalog_fixture *fixture = new_fixture(test, 2);
	struct drm_constraints_entry *target = new_entry(test, fixture, 19);
	struct task_struct *accepting, *closing;
	struct close_race race = { .catalog = fixture->catalog, .entry = target };

	init_completion(&race.installing);
	init_completion(&race.finish);
	init_completion(&race.closing);
	init_completion(&race.closed);
	KUNIT_ASSERT_EQ(test, drm_constraints_catalog_add(fixture->catalog, target), 0);
	accepting = kthread_run(accept_worker, &race, "constraints-accept");
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, accepting);
	KUNIT_EXPECT_NE(test, wait_for_completion_timeout(&race.installing, HZ), 0);
	closing = kthread_run(close_worker, &race, "constraints-close");
	if (IS_ERR(closing)) {
		complete(&race.finish);
		kthread_stop(accepting);
		KUNIT_FAIL(test, "cannot start close worker");
		return;
	}
	KUNIT_EXPECT_NE(test, wait_for_completion_timeout(&race.closing, HZ), 0);
	KUNIT_EXPECT_EQ(test, wait_for_completion_timeout(&race.closed, msecs_to_jiffies(20)), 0);
	complete(&race.finish);
	kthread_stop(accepting);
	kthread_stop(closing);
	KUNIT_EXPECT_EQ(test, race.result, 0);
	KUNIT_EXPECT_TRUE(test, completion_done(&race.closed));
	KUNIT_EXPECT_PTR_EQ(test, drm_constraints_catalog_snapshot(fixture->catalog, 0),
			   ERR_PTR(-ESTALE));
}

static void selection_readback_retains_closed_catalog_binding(struct kunit *test)
{
	struct catalog_fixture *fixture = new_fixture(test, 2);
	struct drm_constraints_entry *target = new_entry(test, fixture, 19);
	struct drm_constraints_entry *selected;
	struct install_context context = {};

	KUNIT_ASSERT_EQ(test, drm_constraints_catalog_add(fixture->catalog, target), 0);
	KUNIT_ASSERT_EQ(test,
		drm_constraints_catalog_accept(fixture->catalog, target, validate, &context), 0);
	drm_constraints_catalog_close(fixture->catalog);
	selected = drm_constraints_catalog_selected(fixture->catalog);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_entry, selected), 0);
	KUNIT_EXPECT_PTR_EQ(test, selected, target);
	kunit_release_action(test, put_entry, target);
	kunit_release_action(test, put_catalog, fixture->catalog);
	KUNIT_EXPECT_EQ(test, fixture->released, 0);
	kunit_release_action(test, put_entry, selected);
	KUNIT_EXPECT_EQ(test, fixture->released, 1);
}

static struct kunit_case drm_constraints_catalog_tests[] = {
	KUNIT_CASE(snapshots_retain_immutable_entries),
	KUNIT_CASE(changes_invalidate_expected_generations),
	KUNIT_CASE(suggestions_do_not_select_entries),
	KUNIT_CASE(removing_an_entry_preserves_other_identities),
	KUNIT_CASE(catalogs_reject_foreign_scope_and_overflow),
	KUNIT_CASE(construction_requires_bounded_matching_scope),
	KUNIT_CASE(test_only_does_not_reserve_selection),
	KUNIT_CASE(accepted_selection_survives_withdrawal),
	KUNIT_CASE(failed_installation_preserves_selection),
	KUNIT_CASE(equal_ids_do_not_authorize_foreign_entries),
	KUNIT_CASE(closing_rejects_checked_but_unaccepted_selection),
	KUNIT_CASE(closing_waits_for_irrevocable_acceptance),
	KUNIT_CASE(selection_readback_retains_closed_catalog_binding),
	{}
};

static struct kunit_suite drm_constraints_catalog_test_suite = {
	.name = "drm_constraints_catalog",
	.test_cases = drm_constraints_catalog_tests,
};

kunit_test_suite(drm_constraints_catalog_test_suite);

MODULE_DESCRIPTION("DRM constraints catalog serialization tests");
MODULE_LICENSE("Dual MIT/GPL");
