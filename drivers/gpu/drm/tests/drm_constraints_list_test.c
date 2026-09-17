// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/err.h>
#include <linux/completion.h>
#include <linux/kthread.h>
#include <linux/module.h>
#include <drm/drm_constraints.h>
#include <drm/drm_constraints_list.h>
#include <drm/drm_constraints_entry.h>
#include <drm/drm_fourcc.h>
#include <kunit/test.h>

struct list_fixture {
	struct drm_constraints_domain *domain;
	struct drm_constraints_description *description;
	struct drm_constraints_entry *initial;
	struct drm_constraints_list *list;
	unsigned int released;
	bool inspect_release;
};

static void release_backend(void *data)
{
	struct list_fixture *fixture = data;

	fixture->released++;
	if (fixture->inspect_release)
		drm_constraints_entry_put(drm_constraints_list_selected(fixture->list));
}

static const struct drm_constraints_entry_ops ops = {
	.owner = THIS_MODULE,
	.release = release_backend,
};

static void put_domain(void *data) { drm_constraints_domain_put(data); }
static void put_description(void *data) { drm_constraints_description_put(data); }
static void put_entry(void *data) { drm_constraints_entry_put(data); }
static void put_list(void *data) { drm_constraints_list_put(data); }
static void put_snapshot(void *data) { drm_constraints_snapshot_put(data); }

static struct drm_constraints_entry *
new_entry(struct kunit *test, struct list_fixture *fixture, u32 crtc_id)
{
	struct drm_constraints_entry *entry;

	entry = drm_constraints_entry_create(fixture->domain, crtc_id,
					     fixture->description, &ops, fixture);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, entry);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_entry, entry), 0);
	return entry;
}

static struct list_fixture *new_fixture(struct kunit *test, unsigned int limit)
{
	const struct drm_constraints_size size = { 64, 32, 64, 32 };
	const struct drm_constraints_format format = {
		.plane_id = 7,
		.format = DRM_FORMAT_XRGB8888,
		.modifier = DRM_FORMAT_MOD_LINEAR,
		.size = size,
	};
	struct list_fixture *fixture = kunit_kzalloc(test, sizeof(*fixture), GFP_KERNEL);

	KUNIT_ASSERT_NOT_NULL(test, fixture);
	fixture->domain = drm_constraints_domain_create(DRM_CONSTRAINTS_MAX_ENTRIES);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, fixture->domain);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_domain, fixture->domain), 0);
	fixture->description = drm_constraints_description_create(&size, &format, 1, NULL, 0);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, fixture->description);
	KUNIT_ASSERT_EQ(test,
		kunit_add_action_or_reset(test, put_description, fixture->description), 0);
	fixture->initial = new_entry(test, fixture, 19);
	fixture->list = drm_constraints_list_create(fixture->domain, fixture->initial, limit);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, fixture->list);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_list, fixture->list), 0);
	return fixture;
}

static struct drm_constraints_snapshot *
snapshot(struct kunit *test, struct drm_constraints_list *list, u64 generation)
{
	struct drm_constraints_snapshot *snapshot;

	snapshot = drm_constraints_list_snapshot(list, generation);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, snapshot);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_snapshot, snapshot), 0);
	return snapshot;
}

static void snapshots_retain_immutable_entries(struct kunit *test)
{
	struct list_fixture *fixture = new_fixture(test, 2);
	struct drm_constraints_snapshot *before = snapshot(test, fixture->list, 0);
	const struct drm_constraints_listing *entries = drm_constraints_snapshot_entries(before);
	u64 id = drm_constraints_entry_id(fixture->initial);

	KUNIT_ASSERT_EQ(test, drm_constraints_list_withdraw(fixture->list, id), 0);
	KUNIT_EXPECT_TRUE(test, entries[0].selectable);
	KUNIT_EXPECT_EQ(test, drm_constraints_snapshot_info(before)->generation, 1);
	kunit_release_action(test, put_list, fixture->list);
	kunit_release_action(test, put_entry, fixture->initial);
	KUNIT_EXPECT_EQ(test, fixture->released, 0);
	KUNIT_EXPECT_EQ(test, drm_constraints_entry_id(entries[0].entry), id);
	kunit_release_action(test, put_snapshot, before);
	KUNIT_EXPECT_EQ(test, fixture->released, 1);
}

static void changes_invalidate_expected_generations(struct kunit *test)
{
	struct list_fixture *fixture = new_fixture(test, 2);
	struct drm_constraints_entry *target = new_entry(test, fixture, 19);
	struct drm_constraints_snapshot *before = snapshot(test, fixture->list, 0);
	u64 generation = drm_constraints_snapshot_info(before)->generation;

	snapshot(test, fixture->list, generation);
	KUNIT_ASSERT_EQ(test, drm_constraints_list_add(fixture->list, target), 0);
	KUNIT_EXPECT_PTR_EQ(test,
		drm_constraints_list_snapshot(fixture->list, generation), ERR_PTR(-ESTALE));
	KUNIT_EXPECT_EQ(test,
		drm_constraints_snapshot_info(snapshot(test, fixture->list, 0))->count, 2);
}

static void suggestions_do_not_select_entries(struct kunit *test)
{
	struct list_fixture *fixture = new_fixture(test, 2);
	struct drm_constraints_entry *target = new_entry(test, fixture, 19);
	const struct drm_constraints_snapshot_info *info;
	u64 id = drm_constraints_entry_id(target), generation;

	KUNIT_ASSERT_EQ(test, drm_constraints_list_add(fixture->list, target), 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_list_suggest(fixture->list, id), 0);
	info = drm_constraints_snapshot_info(snapshot(test, fixture->list, 0));
	KUNIT_EXPECT_EQ(test, info->selected_id, drm_constraints_entry_id(fixture->initial));
	KUNIT_EXPECT_EQ(test, info->suggested_id, id);
	generation = info->generation;
	KUNIT_ASSERT_EQ(test, drm_constraints_list_suggest(fixture->list, id), 0);
	snapshot(test, fixture->list, generation);
	KUNIT_ASSERT_EQ(test, drm_constraints_list_withdraw(fixture->list, id), 0);
	info = drm_constraints_snapshot_info(snapshot(test, fixture->list, 0));
	KUNIT_EXPECT_EQ(test, info->suggested_id, 0);
	KUNIT_EXPECT_EQ(test, info->generation, generation + 1);
	KUNIT_EXPECT_EQ(test, drm_constraints_list_suggest(fixture->list, id), -ESTALE);
	KUNIT_ASSERT_EQ(test, drm_constraints_list_withdraw(fixture->list, id), 0);
	snapshot(test, fixture->list, info->generation);
}

static void removing_an_entry_preserves_other_identities(struct kunit *test)
{
	struct list_fixture *fixture = new_fixture(test, 3);
	struct drm_constraints_entry *first = new_entry(test, fixture, 19);
	struct drm_constraints_entry *second = new_entry(test, fixture, 19);
	struct drm_constraints_snapshot *before, *after;
	u64 first_id = drm_constraints_entry_id(first);

	KUNIT_ASSERT_EQ(test, drm_constraints_list_add(fixture->list, first), 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_list_add(fixture->list, second), 0);
	before = snapshot(test, fixture->list, 0);
	KUNIT_EXPECT_EQ(test, drm_constraints_list_forget(fixture->list, first_id), -EBUSY);
	KUNIT_ASSERT_EQ(test, drm_constraints_list_withdraw(fixture->list, first_id), 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_list_forget(fixture->list, first_id), 0);
	after = snapshot(test, fixture->list, 0);
	KUNIT_EXPECT_EQ(test, drm_constraints_snapshot_info(before)->count, 3);
	KUNIT_EXPECT_EQ(test, drm_constraints_snapshot_info(after)->count, 2);
	KUNIT_EXPECT_PTR_EQ(test, drm_constraints_snapshot_entries(before)[2].entry,
			   drm_constraints_snapshot_entries(after)[1].entry);
	KUNIT_EXPECT_EQ(test, drm_constraints_list_forget(fixture->list,
					   drm_constraints_entry_id(fixture->initial)), -EBUSY);
}

static void lists_reject_foreign_scope_and_overflow(struct kunit *test)
{
	struct list_fixture *fixture = new_fixture(test, 1);
	struct list_fixture *other = new_fixture(test, 1);
	struct drm_constraints_entry *wrong_output = new_entry(test, fixture, 23);
	struct drm_constraints_entry *target = new_entry(test, fixture, 19);

	KUNIT_EXPECT_EQ(test, drm_constraints_list_add(fixture->list, other->initial), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_constraints_list_add(fixture->list, wrong_output), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_constraints_list_add(fixture->list, target), -ENOSPC);
	KUNIT_EXPECT_EQ(test, drm_constraints_list_add(fixture->list, fixture->initial), -EEXIST);
	KUNIT_EXPECT_EQ(test, drm_constraints_list_withdraw(fixture->list, 0), -ENOENT);
	snapshot(test, fixture->list, 1);
}

static void construction_requires_bounded_matching_scope(struct kunit *test)
{
	struct list_fixture *fixture = new_fixture(test, 1);
	struct list_fixture *other = new_fixture(test, 1);

	KUNIT_EXPECT_PTR_EQ(test,
		drm_constraints_list_create(fixture->domain, fixture->initial, 0),
		ERR_PTR(-EINVAL));
	KUNIT_EXPECT_PTR_EQ(test,
		drm_constraints_list_create(fixture->domain, fixture->initial,
					       DRM_CONSTRAINTS_MAX_ENTRIES + 1), ERR_PTR(-EINVAL));
	KUNIT_EXPECT_PTR_EQ(test,
		drm_constraints_list_create(fixture->domain, other->initial, 1),
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
	struct list_fixture *fixture = new_fixture(test, 2);
	struct drm_constraints_entry *target = new_entry(test, fixture, 19);
	struct install_context context = {};
	u64 id = drm_constraints_entry_id(target);

	KUNIT_ASSERT_EQ(test, drm_constraints_list_add(fixture->list, target), 0);
	KUNIT_ASSERT_EQ(test,
		drm_constraints_list_check(fixture->list, target, validate, &context), 0);
	KUNIT_EXPECT_EQ(test,
		drm_constraints_snapshot_info(snapshot(test, fixture->list, 0))->selected_id,
		drm_constraints_entry_id(fixture->initial));
	KUNIT_ASSERT_EQ(test, drm_constraints_list_withdraw(fixture->list, id), 0);
	KUNIT_EXPECT_EQ(test,
		drm_constraints_list_accept(fixture->list, target, install, &context), -ESTALE);
	KUNIT_EXPECT_EQ(test, context.calls, 1);
	KUNIT_EXPECT_PTR_EQ(test, context.accepted, NULL);
}

static void accepted_selection_survives_withdrawal(struct kunit *test)
{
	struct list_fixture *fixture = new_fixture(test, 2);
	struct drm_constraints_entry *target = new_entry(test, fixture, 19);
	struct install_context context = {};
	const struct drm_constraints_snapshot_info *info;
	u64 id = drm_constraints_entry_id(target), generation;

	KUNIT_ASSERT_EQ(test, drm_constraints_list_add(fixture->list, target), 0);
	KUNIT_ASSERT_EQ(test,
		drm_constraints_list_accept(fixture->list, target, install, &context), 0);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_entry, context.accepted), 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_list_withdraw(fixture->list, id), 0);
	info = drm_constraints_snapshot_info(snapshot(test, fixture->list, 0));
	KUNIT_EXPECT_EQ(test, info->selected_id, id);
	generation = info->generation;
	KUNIT_ASSERT_EQ(test,
		drm_constraints_list_accept(fixture->list, target, validate, &context), 0);
	snapshot(test, fixture->list, generation);
	kunit_release_action(test, put_list, fixture->list);
	KUNIT_EXPECT_EQ(test, drm_constraints_entry_id(context.accepted), id);
}

static void failed_installation_preserves_selection(struct kunit *test)
{
	struct list_fixture *fixture = new_fixture(test, 2);
	struct drm_constraints_entry *target = new_entry(test, fixture, 19);
	struct install_context context = { .result = -ENOMEM };
	const struct drm_constraints_snapshot_info *info;
	u64 generation;

	KUNIT_ASSERT_EQ(test, drm_constraints_list_add(fixture->list, target), 0);
	info = drm_constraints_snapshot_info(snapshot(test, fixture->list, 0));
	generation = info->generation;
	KUNIT_EXPECT_EQ(test,
		drm_constraints_list_accept(fixture->list, target, install, &context), -ENOMEM);
	info = drm_constraints_snapshot_info(snapshot(test, fixture->list, generation));
	KUNIT_EXPECT_EQ(test, info->selected_id, drm_constraints_entry_id(fixture->initial));
	KUNIT_EXPECT_PTR_EQ(test, context.accepted, NULL);
	context.result = 1;
	KUNIT_EXPECT_EQ(test,
		drm_constraints_list_accept(fixture->list, target, install, &context), -EINVAL);
	snapshot(test, fixture->list, generation);
}

static void equal_ids_do_not_authorize_foreign_entries(struct kunit *test)
{
	struct list_fixture *fixture = new_fixture(test, 1);
	struct list_fixture *other = new_fixture(test, 1);
	struct install_context context = {};

	KUNIT_ASSERT_EQ(test, drm_constraints_entry_id(fixture->initial),
			drm_constraints_entry_id(other->initial));
	KUNIT_EXPECT_EQ(test,
		drm_constraints_list_accept(fixture->list, other->initial, install, &context),
		-ESTALE);
	KUNIT_EXPECT_EQ(test, context.calls, 0);
}

static void closing_rejects_checked_but_unaccepted_selection(struct kunit *test)
{
	struct list_fixture *fixture = new_fixture(test, 1);
	struct install_context context = {};
	struct drm_constraints_snapshot *before = snapshot(test, fixture->list, 0);

	KUNIT_ASSERT_EQ(test,
		drm_constraints_list_check(fixture->list, fixture->initial, validate, &context), 0);
	drm_constraints_list_close(fixture->list);
	drm_constraints_list_close(fixture->list);
	KUNIT_EXPECT_EQ(test,
		drm_constraints_list_accept(fixture->list, fixture->initial, install, &context),
		-ESTALE);
	KUNIT_EXPECT_EQ(test, context.calls, 1);
	KUNIT_EXPECT_PTR_EQ(test, drm_constraints_list_snapshot(fixture->list, 0),
			   ERR_PTR(-ESTALE));
	KUNIT_EXPECT_EQ(test, drm_constraints_list_add(fixture->list, fixture->initial), -ESTALE);
	KUNIT_EXPECT_EQ(test, drm_constraints_list_suggest(fixture->list, 0), -ESTALE);
	KUNIT_EXPECT_EQ(test, drm_constraints_snapshot_info(before)->selected_id,
			drm_constraints_entry_id(fixture->initial));
}

struct close_race {
	struct drm_constraints_list *list;
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

	race->result = drm_constraints_list_accept(race->list, race->entry,
						      paused_install, race);
	while (!kthread_should_stop())
		schedule_timeout_interruptible(1);
	return 0;
}

static int close_worker(void *data)
{
	struct close_race *race = data;

	complete(&race->closing);
	drm_constraints_list_close(race->list);
	complete(&race->closed);
	while (!kthread_should_stop())
		schedule_timeout_interruptible(1);
	return 0;
}

static void closing_waits_for_irrevocable_acceptance(struct kunit *test)
{
	struct list_fixture *fixture = new_fixture(test, 2);
	struct drm_constraints_entry *target = new_entry(test, fixture, 19);
	struct task_struct *accepting, *closing;
	struct close_race race = { .list = fixture->list, .entry = target };

	init_completion(&race.installing);
	init_completion(&race.finish);
	init_completion(&race.closing);
	init_completion(&race.closed);
	KUNIT_ASSERT_EQ(test, drm_constraints_list_add(fixture->list, target), 0);
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
	KUNIT_EXPECT_PTR_EQ(test, drm_constraints_list_snapshot(fixture->list, 0),
			   ERR_PTR(-ESTALE));
}

static void selection_readback_retains_closed_list_binding(struct kunit *test)
{
	struct list_fixture *fixture = new_fixture(test, 2);
	struct drm_constraints_entry *target = new_entry(test, fixture, 19);
	struct drm_constraints_entry *selected;
	struct install_context context = {};

	KUNIT_ASSERT_EQ(test, drm_constraints_list_add(fixture->list, target), 0);
	KUNIT_ASSERT_EQ(test,
		drm_constraints_list_accept(fixture->list, target, validate, &context), 0);
	drm_constraints_list_close(fixture->list);
	selected = drm_constraints_list_selected(fixture->list);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_entry, selected), 0);
	KUNIT_EXPECT_PTR_EQ(test, selected, target);
	kunit_release_action(test, put_entry, target);
	kunit_release_action(test, put_list, fixture->list);
	KUNIT_EXPECT_EQ(test, fixture->released, 0);
	kunit_release_action(test, put_entry, selected);
	KUNIT_EXPECT_EQ(test, fixture->released, 1);
}

static void identity_lookup_retains_without_reserving_selection(struct kunit *test)
{
	struct list_fixture *fixture = new_fixture(test, 2);
	struct drm_constraints_entry *target = new_entry(test, fixture, 19);
	struct drm_constraints_entry *retained;
	struct install_context context = {};
	u64 id = drm_constraints_entry_id(target);

	KUNIT_EXPECT_PTR_EQ(test, drm_constraints_list_lookup(fixture->list, 0),
			   ERR_PTR(-EINVAL));
	KUNIT_EXPECT_PTR_EQ(test, drm_constraints_list_lookup(fixture->list, id),
			   ERR_PTR(-ESTALE));
	KUNIT_EXPECT_PTR_EQ(test, drm_constraints_list_lookup(fixture->list, U64_MAX),
			   ERR_PTR(-ESTALE));
	KUNIT_ASSERT_EQ(test, drm_constraints_list_add(fixture->list, target), 0);
	retained = drm_constraints_list_lookup(fixture->list, id);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, retained);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_entry, retained), 0);
	KUNIT_EXPECT_PTR_EQ(test, retained, target);
	KUNIT_ASSERT_EQ(test, drm_constraints_list_withdraw(fixture->list, id), 0);
	KUNIT_EXPECT_PTR_EQ(test, drm_constraints_list_lookup(fixture->list, id),
			   ERR_PTR(-ESTALE));
	KUNIT_EXPECT_EQ(test,
		drm_constraints_list_accept(fixture->list, retained, validate, &context),
		-ESTALE);
	KUNIT_ASSERT_EQ(test, drm_constraints_list_forget(fixture->list, id), 0);
	kunit_release_action(test, put_entry, target);
	KUNIT_EXPECT_EQ(test, fixture->released, 0);
	KUNIT_EXPECT_EQ(test, drm_constraints_entry_id(retained), id);
	kunit_release_action(test, put_entry, retained);
	KUNIT_EXPECT_EQ(test, fixture->released, 1);
}

static void identity_lookup_observes_selected_withdrawal_and_closure(struct kunit *test)
{
	struct list_fixture *fixture = new_fixture(test, 1);
	struct drm_constraints_entry *selected;
	u64 id = drm_constraints_entry_id(fixture->initial);

	KUNIT_ASSERT_EQ(test, drm_constraints_list_withdraw(fixture->list, id), 0);
	selected = drm_constraints_list_lookup(fixture->list, id);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, selected);
	KUNIT_EXPECT_PTR_EQ(test, selected, fixture->initial);
	drm_constraints_entry_put(selected);
	drm_constraints_list_close(fixture->list);
	KUNIT_EXPECT_PTR_EQ(test, drm_constraints_list_lookup(fixture->list, id),
			   ERR_PTR(-ESTALE));
}

static void retiring_offers_preserves_snapshots_and_default(struct kunit *test)
{
	struct list_fixture *fixture = new_fixture(test, 3);
	struct drm_constraints_entry *first = new_entry(test, fixture, 19);
	struct drm_constraints_entry *second = new_entry(test, fixture, 19);
	struct drm_constraints_snapshot *before, *after;
	const struct drm_constraints_snapshot_info *info;
	u64 generation, id = drm_constraints_entry_id(first);

	KUNIT_ASSERT_EQ(test, drm_constraints_list_add(fixture->list, first), 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_list_add(fixture->list, second), 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_list_suggest(fixture->list, id), 0);
	before = snapshot(test, fixture->list, 0);
	generation = drm_constraints_snapshot_info(before)->generation;
	kunit_release_action(test, put_entry, first);
	kunit_release_action(test, put_entry, second);
	KUNIT_ASSERT_EQ(test,
		drm_constraints_list_retain_default(fixture->list, fixture->initial), 0);
	after = snapshot(test, fixture->list, 0);
	info = drm_constraints_snapshot_info(after);
	KUNIT_EXPECT_EQ(test, info->count, 1);
	KUNIT_EXPECT_EQ(test, info->selected_id, drm_constraints_entry_id(fixture->initial));
	KUNIT_EXPECT_EQ(test, info->suggested_id, 0);
	KUNIT_EXPECT_EQ(test, info->generation, generation + 1);
	KUNIT_EXPECT_EQ(test, drm_constraints_snapshot_info(before)->count, 3);
	KUNIT_EXPECT_EQ(test, fixture->released, 0);
	KUNIT_EXPECT_PTR_EQ(test, drm_constraints_list_lookup(fixture->list, id),
			   ERR_PTR(-ESTALE));
	KUNIT_ASSERT_EQ(test,
		drm_constraints_list_retain_default(fixture->list, fixture->initial), 0);
	snapshot(test, fixture->list, info->generation);
	kunit_release_action(test, put_snapshot, before);
	KUNIT_EXPECT_EQ(test, fixture->released, 2);
}

static void retiring_offers_releases_resources_outside_list_lock(struct kunit *test)
{
	struct list_fixture *fixture = new_fixture(test, 2);
	struct drm_constraints_entry *target = new_entry(test, fixture, 19);

	KUNIT_ASSERT_EQ(test, drm_constraints_list_add(fixture->list, target), 0);
	kunit_release_action(test, put_entry, target);
	fixture->inspect_release = true;
	KUNIT_EXPECT_EQ(test,
		drm_constraints_list_retain_default(fixture->list, fixture->initial), 0);
	fixture->inspect_release = false;
	KUNIT_EXPECT_EQ(test, fixture->released, 1);
}

static void retiring_offers_requires_an_available_selected_default(struct kunit *test)
{
	struct list_fixture *fixture = new_fixture(test, 2);
	struct drm_constraints_entry *target = new_entry(test, fixture, 19);
	struct install_context context = {};
	const struct drm_constraints_snapshot_info *info;
	u64 generation;

	KUNIT_ASSERT_EQ(test, drm_constraints_list_add(fixture->list, target), 0);
	KUNIT_ASSERT_EQ(test,
		drm_constraints_list_accept(fixture->list, target, validate, &context), 0);
	info = drm_constraints_snapshot_info(snapshot(test, fixture->list, 0));
	generation = info->generation;
	KUNIT_EXPECT_EQ(test,
		drm_constraints_list_retain_default(fixture->list, fixture->initial), -EBUSY);
	snapshot(test, fixture->list, generation);
	KUNIT_ASSERT_EQ(test,
		drm_constraints_list_accept(fixture->list, fixture->initial,
					    validate, &context), 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_list_withdraw(fixture->list,
						 drm_constraints_entry_id(fixture->initial)), 0);
	KUNIT_EXPECT_EQ(test,
		drm_constraints_list_retain_default(fixture->list, fixture->initial), -ESTALE);
	drm_constraints_list_close(fixture->list);
	KUNIT_EXPECT_EQ(test,
		drm_constraints_list_retain_default(fixture->list, target), -ESTALE);
}

struct list_observer {
	wait_queue_entry_t wait;
	struct drm_constraints_list *list;
	unsigned int notifications;
};

static int observe_wakeup(wait_queue_entry_t *wait, unsigned int mode, int flags, void *key)
{
	struct list_observer *observer = container_of(wait, struct list_observer, wait);

	observer->notifications++;
	return 0;
}

static void remove_observer(void *data)
{
	struct list_observer *observer = data;

	remove_wait_queue(drm_constraints_list_waitqueue(observer->list), &observer->wait);
	drm_constraints_list_put(observer->list);
}

static struct list_observer *observe_list(struct kunit *test, struct drm_constraints_list *list)
{
	struct list_observer *observer = kunit_kzalloc(test, sizeof(*observer), GFP_KERNEL);

	KUNIT_ASSERT_NOT_NULL(test, observer);
	observer->list = drm_constraints_list_get(list);
	init_waitqueue_func_entry(&observer->wait, observe_wakeup);
	add_wait_queue(drm_constraints_list_waitqueue(list), &observer->wait);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, remove_observer, observer), 0);
	return observer;
}

static int accept_observed(struct drm_constraints_entry *entry, void *data)
{
	return 0;
}

static void generation_observers_follow_every_visible_change(struct kunit *test)
{
	struct list_fixture *fixture = new_fixture(test, 3);
	struct drm_constraints_entry *target = new_entry(test, fixture, 19);
	struct drm_constraints_list *list = fixture->list;
	struct list_observer *observer = observe_list(test, list);
	u64 id = drm_constraints_entry_id(target), generation = 0;

	KUNIT_ASSERT_EQ(test, drm_constraints_list_observe(list, &generation), 0);
	KUNIT_EXPECT_EQ(test, generation, 1);
	KUNIT_EXPECT_EQ(test, observer->notifications, 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_list_add(list, target), 0);
	KUNIT_EXPECT_EQ(test, observer->notifications, 1);
	KUNIT_ASSERT_EQ(test, drm_constraints_list_suggest(list, id), 0);
	KUNIT_EXPECT_EQ(test, observer->notifications, 2);
	KUNIT_ASSERT_EQ(test, drm_constraints_list_accept(list, target, accept_observed, NULL), 0);
	KUNIT_EXPECT_EQ(test, observer->notifications, 3);
	KUNIT_ASSERT_EQ(test, drm_constraints_list_withdraw(list, id), 0);
	KUNIT_EXPECT_EQ(test, observer->notifications, 4);
	KUNIT_ASSERT_EQ(test,
		drm_constraints_list_accept(list, fixture->initial, accept_observed, NULL), 0);
	KUNIT_EXPECT_EQ(test, observer->notifications, 5);
	KUNIT_ASSERT_EQ(test, drm_constraints_list_forget(list, id), 0);
	KUNIT_EXPECT_EQ(test, observer->notifications, 6);
	KUNIT_ASSERT_EQ(test, drm_constraints_list_add(list, target), 0);
	KUNIT_EXPECT_EQ(test, observer->notifications, 7);
	KUNIT_ASSERT_EQ(test, drm_constraints_list_retain_default(list, fixture->initial), 0);
	KUNIT_EXPECT_EQ(test, observer->notifications, 8);
	KUNIT_ASSERT_EQ(test, drm_constraints_list_observe(list, &generation), 0);
	KUNIT_EXPECT_EQ(test, generation, 9);
	KUNIT_EXPECT_EQ(test,
		drm_constraints_snapshot_info(snapshot(test, list, generation))->generation,
		generation);
}

static void failed_or_repeated_operations_do_not_notify(struct kunit *test)
{
	struct list_fixture *fixture = new_fixture(test, 2);
	struct list_observer *observer = observe_list(test, fixture->list);
	u64 id = drm_constraints_entry_id(fixture->initial);

	KUNIT_EXPECT_EQ(test, drm_constraints_list_add(fixture->list, fixture->initial), -EEXIST);
	KUNIT_EXPECT_EQ(test, drm_constraints_list_suggest(fixture->list, 0), 0);
	KUNIT_EXPECT_EQ(test, drm_constraints_list_suggest(fixture->list, id + 1), -ESTALE);
	KUNIT_EXPECT_EQ(test,
		drm_constraints_list_accept(fixture->list, fixture->initial,
					    accept_observed, NULL), 0);
	KUNIT_EXPECT_EQ(test,
		drm_constraints_list_retain_default(fixture->list, fixture->initial), 0);
	KUNIT_EXPECT_EQ(test, observer->notifications, 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_list_withdraw(fixture->list, id), 0);
	KUNIT_EXPECT_EQ(test, observer->notifications, 1);
	KUNIT_EXPECT_EQ(test, drm_constraints_list_withdraw(fixture->list, id), 0);
	KUNIT_EXPECT_EQ(test, observer->notifications, 1);
}

static void closure_wakes_retained_observers_once(struct kunit *test)
{
	struct list_fixture *fixture = new_fixture(test, 2);
	struct list_observer *observer = observe_list(test, fixture->list);
	u64 generation = 99;

	kunit_release_action(test, put_list, fixture->list);
	drm_constraints_list_close(observer->list);
	KUNIT_EXPECT_EQ(test, observer->notifications, 1);
	KUNIT_EXPECT_EQ(test, drm_constraints_list_observe(observer->list, &generation), -ESTALE);
	KUNIT_EXPECT_EQ(test, generation, 99);
	drm_constraints_list_close(observer->list);
	KUNIT_EXPECT_EQ(test, observer->notifications, 1);
}

static struct kunit_case drm_constraints_list_tests[] = {
	KUNIT_CASE(generation_observers_follow_every_visible_change),
	KUNIT_CASE(failed_or_repeated_operations_do_not_notify),
	KUNIT_CASE(closure_wakes_retained_observers_once),
	KUNIT_CASE(retiring_offers_preserves_snapshots_and_default),
	KUNIT_CASE(retiring_offers_releases_resources_outside_list_lock),
	KUNIT_CASE(retiring_offers_requires_an_available_selected_default),
	KUNIT_CASE(snapshots_retain_immutable_entries),
	KUNIT_CASE(changes_invalidate_expected_generations),
	KUNIT_CASE(suggestions_do_not_select_entries),
	KUNIT_CASE(removing_an_entry_preserves_other_identities),
	KUNIT_CASE(lists_reject_foreign_scope_and_overflow),
	KUNIT_CASE(construction_requires_bounded_matching_scope),
	KUNIT_CASE(test_only_does_not_reserve_selection),
	KUNIT_CASE(accepted_selection_survives_withdrawal),
	KUNIT_CASE(failed_installation_preserves_selection),
	KUNIT_CASE(equal_ids_do_not_authorize_foreign_entries),
	KUNIT_CASE(closing_rejects_checked_but_unaccepted_selection),
	KUNIT_CASE(closing_waits_for_irrevocable_acceptance),
	KUNIT_CASE(selection_readback_retains_closed_list_binding),
	KUNIT_CASE(identity_lookup_retains_without_reserving_selection),
	KUNIT_CASE(identity_lookup_observes_selected_withdrawal_and_closure),
	{}
};

static struct kunit_suite drm_constraints_list_test_suite = {
	.name = "drm_constraints_list",
	.test_cases = drm_constraints_list_tests,
};

kunit_test_suite(drm_constraints_list_test_suite);

MODULE_DESCRIPTION("DRM constraints list serialization tests");
MODULE_LICENSE("Dual MIT/GPL");
