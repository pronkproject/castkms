// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <drm/drm_atomic.h>
#include <drm/drm_atomic_helper.h>
#include <drm/drm_atomic_prepare.h>
#include <drm/drm_atomic_prepare_display.h>
#include <drm/drm_atomic_prepare_file.h>
#include <drm/drm_atomic_prepare_outputs.h>
#include <drm/drm_atomic_prepare_owner.h>
#include <drm/drm_atomic_prepare_submission.h>
#include <drm/drm_atomic_prepare_ticket.h>
#include <drm/drm_crtc.h>
#include <drm/drm_kunit_helpers.h>
#include <kunit/test.h>
#include <linux/file.h>
#include <linux/poll.h>

struct display_fixture {
	struct drm_device *dev;
	struct drm_crtc *crtc;
	struct drm_prepare_read_claim *read;
};

static void release_display(void *data)
{
	struct display_fixture *f = data;

	if (f->read)
		drm_prepare_read_abandon(f->read);
}

static struct display_fixture *new_display(struct kunit *test, bool enabled)
{
	struct device *parent = drm_kunit_helper_alloc_device(test);
	struct display_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct drm_plane *plane;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	KUNIT_ASSERT_NOT_NULL(test, f);
	f->dev = __drm_kunit_helper_alloc_drm_device(test, parent,
				sizeof(*f->dev), 0, DRIVER_MODESET | DRIVER_ATOMIC);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->dev);
	if (enabled)
		KUNIT_ASSERT_EQ(test, drm_atomic_prepare_display_init(f->dev, 8), 0);
	plane = drm_kunit_helper_create_primary_plane(test, f->dev, NULL, NULL, NULL, 0, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, plane);
	f->crtc = drm_kunit_helper_create_crtc(test, f->dev, plane, NULL, NULL, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->crtc);
	drm_mode_config_reset(f->dev);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, release_display, f), 0);
	return f;
}

/* No fixture state is published to another task between these operations. */
static int lock_update(struct drm_atomic_commit *state, struct drm_modeset_acquire_ctx *ctx)
{
	int ret;

	drm_modeset_acquire_init(ctx, 0);
	state->acquire_ctx = ctx;
	for (;;) {
		ret = drm_modeset_lock_all_ctx(state->dev, ctx);
		if (ret != -EDEADLK)
			return ret;
		ret = drm_modeset_backoff(ctx);
		if (ret)
			return ret;
	}
}

static void unlock_update(struct drm_atomic_commit *state)
{
	drm_modeset_drop_locks(state->acquire_ctx);
	drm_modeset_acquire_fini(state->acquire_ctx);
	state->acquire_ctx = NULL;
}

static int run_update(struct drm_atomic_commit *state, int (*operation)(struct drm_atomic_commit *))
{
	struct drm_modeset_acquire_ctx ctx;
	int ret = lock_update(state, &ctx);

	if (!ret)
		ret = operation(state);
	unlock_update(state);
	return ret;
}

static int swap_update(struct drm_atomic_commit *state)
{
	return drm_atomic_helper_swap_state(state, false);
}

static struct drm_crtc_state *get_update_crtc(struct drm_atomic_commit *state, struct drm_crtc *crtc)
{
	struct drm_modeset_acquire_ctx ctx;
	int ret = lock_update(state, &ctx);
	struct drm_crtc_state *crtc_state;

	crtc_state = ret ? ERR_PTR(ret) : drm_atomic_get_crtc_state(state, crtc);
	unlock_update(state);
	return crtc_state;
}

static int observe_update(struct drm_atomic_commit *state,
			  struct drm_prepare_output_generation *entries, unsigned int capacity)
{
	struct drm_modeset_acquire_ctx ctx;
	int ret = lock_update(state, &ctx);

	if (!ret)
		ret = drm_atomic_prepare_display_observe(state, entries, capacity);
	unlock_update(state);
	return ret;
}

static struct drm_prepare_source *display_source(struct drm_crtc *crtc)
{
	struct drm_prepare_source *source;
	int ret = drm_modeset_lock(&crtc->mutex, NULL);

	if (ret)
		return ERR_PTR(ret);
	source = drm_atomic_prepare_crtc_source(crtc);
	drm_modeset_unlock(&crtc->mutex);
	return source;
}

static struct drm_atomic_commit *new_update(struct kunit *test, struct display_fixture *f)
{
	struct drm_atomic_commit *state = drm_kunit_helper_atomic_state_alloc(test, f->dev, NULL);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, get_update_crtc(state, f->crtc));
	return state;
}

static void unchanged_blank_update_has_distinct_generation(struct kunit *test)
{
	struct display_fixture *f = new_display(test, true);
	struct drm_atomic_commit *state = new_update(test, f);
	struct drm_prepare_output_generation observed;
	struct drm_prepare_source *before = display_source(f->crtc);
	struct drm_crtc_state *next = drm_atomic_get_new_crtc_state(state, f->crtc);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, before);
	KUNIT_EXPECT_PTR_EQ(test, next->prepare_source, NULL);
	KUNIT_ASSERT_EQ(test, run_update(state, drm_atomic_check_only), 0);
	KUNIT_ASSERT_NOT_NULL(test, next->prepare_source);
	KUNIT_EXPECT_PTR_NE(test, next->prepare_source, before);
	KUNIT_EXPECT_PTR_EQ(test, display_source(f->crtc), before);
	KUNIT_ASSERT_EQ(test, observe_update(state, &observed, 1), 1);
	KUNIT_EXPECT_EQ(test, observed.crtc_id, f->crtc->base.id);
	KUNIT_EXPECT_PTR_EQ(test, observed.source, before);
	KUNIT_EXPECT_EQ(test, observe_update(state, NULL, 0), -E2BIG);
	KUNIT_ASSERT_EQ(test, run_update(state, swap_update), 0);
	KUNIT_EXPECT_PTR_EQ(test, display_source(f->crtc), next->prepare_source);
	drm_atomic_commit_clear(state);
}

static void discarded_check_preserves_accepted_generation(struct kunit *test)
{
	struct display_fixture *f = new_display(test, true);
	struct drm_atomic_commit *state = new_update(test, f);
	struct drm_prepare_source *before = display_source(f->crtc);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, before);
	KUNIT_ASSERT_EQ(test, run_update(state, drm_atomic_check_only), 0);
	drm_atomic_commit_clear(state);
	KUNIT_EXPECT_PTR_EQ(test, display_source(f->crtc), before);
	KUNIT_EXPECT_EQ(test, drm_atomic_prepare_display_init(f->dev, 8), -EBUSY);
}

static void ordinary_device_does_not_allocate_generations(struct kunit *test)
{
	struct display_fixture *f = new_display(test, false);
	struct drm_atomic_commit *state = new_update(test, f);

	KUNIT_ASSERT_EQ(test, run_update(state, drm_atomic_check_only), 0);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->prepare_source, NULL);
	KUNIT_EXPECT_PTR_EQ(test, drm_atomic_get_new_crtc_state(state, f->crtc)->prepare_source, NULL);
	KUNIT_EXPECT_EQ(test, PTR_ERR(display_source(f->crtc)), -EOPNOTSUPP);
	drm_atomic_commit_clear(state);
}

static void release_owner(void *data)
{
	drm_prepare_owner_put(data);
}

static void release_ticket(void *data)
{
	drm_prepare_ticket_put(data);
}

static void release_ticket_file(void *data)
{
	__fput_sync(data);
}

static void free_poll_wait(void *data)
{
	poll_freewait(data);
}

static void claim_display(struct kunit *test, struct display_fixture *f,
			  struct drm_prepare_source *source)
{
	struct drm_prepare_read_claim *read = drm_prepare_source_claim(source);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, read);
	f->read = read;
}

static struct poll_wqueues *watch_ticket(struct kunit *test, struct drm_prepare_ticket *ticket)
{
	struct file *file = drm_prepare_ticket_file_create(ticket);
	struct poll_wqueues *wait = kunit_kzalloc(test, sizeof(*wait), GFP_KERNEL);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, file);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, release_ticket_file, file), 0);
	KUNIT_ASSERT_NOT_NULL(test, wait);
	poll_initwait(wait);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, free_poll_wait, wait), 0);
	KUNIT_ASSERT_EQ(test, vfs_poll(file, &wait->pt), 0);
	return wait;
}

static struct drm_prepare_owner *new_owner(struct kunit *test)
{
	struct drm_prepare_owner *owner = drm_prepare_owner_create(4);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, owner);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, release_owner, owner), 0);
	return owner;
}

static struct drm_prepare_ticket *new_ticket(struct kunit *test,
					    struct display_fixture *f,
					    struct drm_prepare_owner *owner)
{
	struct drm_prepare_ticket *ticket;
	int ret = drm_modeset_lock(&f->crtc->mutex, NULL);

	KUNIT_ASSERT_EQ(test, ret, 0);
	ticket = drm_atomic_prepare_crtcs(&f->crtc, 1, owner);
	drm_modeset_unlock(&f->crtc->mutex);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, ticket);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, release_ticket, ticket), 0);
	return ticket;
}

static void submission_requires_matching_issuer_and_current_generation(struct kunit *test)
{
	struct display_fixture *f = new_display(test, true);
	struct drm_atomic_commit *state = new_update(test, f);
	struct drm_prepare_owner *owner = new_owner(test);
	struct drm_prepare_owner *other = new_owner(test);
	struct drm_prepare_ticket *ticket = new_ticket(test, f, owner);

	KUNIT_EXPECT_EQ(test, drm_atomic_prepare_submission_set(state, f->crtc, ticket),
			-EOPNOTSUPP);
	KUNIT_ASSERT_EQ(test, drm_atomic_prepare_submission_init(state, other), 0);
	KUNIT_EXPECT_EQ(test, drm_atomic_prepare_submission_set(state, f->crtc, ticket), -EACCES);
	drm_atomic_commit_clear(state);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, get_update_crtc(state, f->crtc));
	KUNIT_ASSERT_EQ(test, drm_atomic_prepare_submission_init(state, owner), 0);
	KUNIT_EXPECT_EQ(test, run_update(state, drm_atomic_prepare_submission_attach), -EINVAL);
	KUNIT_ASSERT_EQ(test, drm_atomic_prepare_submission_set(state, f->crtc, ticket), 0);
	KUNIT_EXPECT_EQ(test, drm_atomic_prepare_submission_set(state, f->crtc, ticket), -EINVAL);
	KUNIT_ASSERT_EQ(test, run_update(state, drm_atomic_check_only), 0);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_status(ticket), DRM_PREPARE_TICKET_READY);
	KUNIT_ASSERT_EQ(test, run_update(state, drm_atomic_prepare_submission_attach), 0);
	KUNIT_ASSERT_EQ(test, run_update(state, swap_update), 0);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_status(ticket), DRM_PREPARE_TICKET_CONSUMED);
	drm_atomic_commit_clear(state);
}

static void submission_rejects_generation_replaced_after_issue(struct kunit *test)
{
	struct display_fixture *f = new_display(test, true);
	struct drm_atomic_commit *state = new_update(test, f);
	struct drm_prepare_owner *owner = new_owner(test);
	struct drm_prepare_ticket *ticket = new_ticket(test, f, owner);
	struct drm_prepare_source *accepted;

	KUNIT_ASSERT_EQ(test, run_update(state, drm_atomic_check_only), 0);
	KUNIT_ASSERT_EQ(test, run_update(state, swap_update), 0);
	drm_atomic_commit_clear(state);
	accepted = display_source(f->crtc);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, get_update_crtc(state, f->crtc));
	KUNIT_ASSERT_EQ(test, drm_atomic_prepare_submission_init(state, owner), 0);
	KUNIT_ASSERT_EQ(test, drm_atomic_prepare_submission_set(state, f->crtc, ticket), 0);
	KUNIT_ASSERT_EQ(test, run_update(state, drm_atomic_prepare_submission_attach), 0);
	KUNIT_ASSERT_EQ(test, run_update(state, drm_atomic_check_only), 0);
	KUNIT_EXPECT_EQ(test, run_update(state, swap_update), -ESTALE);
	KUNIT_EXPECT_PTR_EQ(test, display_source(f->crtc), accepted);
	drm_atomic_commit_clear(state);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_status(ticket), DRM_PREPARE_TICKET_READY);
}

static void pending_reader_wakes_ticket_before_acceptance(struct kunit *test)
{
	struct display_fixture *f = new_display(test, true);
	struct drm_atomic_commit *state = new_update(test, f);
	struct drm_prepare_owner *owner = new_owner(test);
	struct drm_prepare_source *source = display_source(f->crtc);
	struct drm_prepare_ticket *ticket;
	struct poll_wqueues *wait;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, source);
	claim_display(test, f, source);
	ticket = new_ticket(test, f, owner);
	wait = watch_ticket(test, ticket);
	KUNIT_ASSERT_EQ(test, drm_atomic_prepare_submission_init(state, owner), 0);
	KUNIT_ASSERT_EQ(test, drm_atomic_prepare_submission_set(state, f->crtc, ticket), 0);
	KUNIT_ASSERT_EQ(test, run_update(state, drm_atomic_check_only), 0);
	KUNIT_EXPECT_EQ(test, run_update(state, drm_atomic_prepare_submission_attach), -EAGAIN);
	KUNIT_EXPECT_PTR_EQ(test, display_source(f->crtc), source);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_status(ticket), DRM_PREPARE_TICKET_PENDING);
	drm_prepare_read_release(f->read, NULL);
	f->read = NULL;
	KUNIT_EXPECT_TRUE(test, wait->triggered);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_status(ticket), DRM_PREPARE_TICKET_READY);
	KUNIT_ASSERT_EQ(test, run_update(state, drm_atomic_prepare_submission_attach), 0);
	KUNIT_ASSERT_EQ(test, run_update(state, swap_update), 0);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_status(ticket), DRM_PREPARE_TICKET_CONSUMED);
	drm_atomic_commit_clear(state);
}

static void revoked_pending_ticket_cannot_accept_after_reader_release(struct kunit *test)
{
	struct display_fixture *f = new_display(test, true);
	struct drm_atomic_commit *state = new_update(test, f);
	struct drm_prepare_owner *owner = new_owner(test);
	struct drm_prepare_source *source = display_source(f->crtc);
	struct drm_prepare_ticket *ticket;
	struct poll_wqueues *wait;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, source);
	claim_display(test, f, source);
	ticket = new_ticket(test, f, owner);
	wait = watch_ticket(test, ticket);
	KUNIT_ASSERT_EQ(test, drm_atomic_prepare_submission_init(state, owner), 0);
	KUNIT_ASSERT_EQ(test, drm_atomic_prepare_submission_set(state, f->crtc, ticket), 0);
	drm_prepare_owner_revoke(owner);
	KUNIT_EXPECT_TRUE(test, wait->triggered);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_status(ticket), DRM_PREPARE_TICKET_CANCELED);
	drm_prepare_read_release(f->read, NULL);
	f->read = NULL;
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_status(ticket), DRM_PREPARE_TICKET_CANCELED);
	KUNIT_EXPECT_EQ(test, run_update(state, drm_atomic_prepare_submission_attach), -ECANCELED);
	KUNIT_EXPECT_PTR_EQ(test, display_source(f->crtc), source);
	drm_atomic_commit_clear(state);
}

static struct kunit_case cases[] = {
	KUNIT_CASE(unchanged_blank_update_has_distinct_generation),
	KUNIT_CASE(discarded_check_preserves_accepted_generation),
	KUNIT_CASE(ordinary_device_does_not_allocate_generations),
	KUNIT_CASE(submission_requires_matching_issuer_and_current_generation),
	KUNIT_CASE(submission_rejects_generation_replaced_after_issue),
	KUNIT_CASE(pending_reader_wakes_ticket_before_acceptance),
	KUNIT_CASE(revoked_pending_ticket_cannot_accept_after_reader_release),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_prepare_display",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_LICENSE("GPL");
