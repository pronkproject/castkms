// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <drm/drm_atomic.h>
#include <drm/drm_atomic_helper.h>
#include <drm/drm_atomic_uapi.h>
#include <drm/drm_kunit_helpers.h>
#include <kunit/test.h>

struct cursor_fixture {
	struct drm_device *dev;
	struct drm_crtc *crtc;
	struct drm_atomic_commit *state;
};

static void finish_fixture(void *data)
{
	struct cursor_fixture *f = data;

	drm_atomic_commit_put(f->state);
}

static struct cursor_fixture *new_fixture(struct kunit *test)
{
	struct cursor_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct device *parent = drm_kunit_helper_alloc_device(test);
	struct drm_plane *plane;

	KUNIT_ASSERT_NOT_NULL(test, f);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	f->dev = __drm_kunit_helper_alloc_drm_device(test, parent, sizeof(*f->dev), 0,
						  DRIVER_MODESET | DRIVER_ATOMIC);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->dev);
	plane = drm_kunit_helper_create_primary_plane(test, f->dev, NULL, NULL, NULL, 0, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, plane);
	f->crtc = drm_kunit_helper_create_crtc(test, f->dev, plane, NULL, NULL, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->crtc);
	drm_mode_config_reset(f->dev);
	f->crtc->cursor_x = 3;
	f->crtc->cursor_y = 5;
	f->state = drm_atomic_commit_alloc(f->dev);
	KUNIT_ASSERT_NOT_NULL(test, f->state);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, finish_fixture, f), 0);
	return f;
}

static int run_update(struct cursor_fixture *f, int (*operation)(struct cursor_fixture *))
{
	struct drm_modeset_acquire_ctx ctx;
	int ret;

	drm_modeset_acquire_init(&ctx, 0);
	f->state->acquire_ctx = &ctx;
	for (;;) {
		ret = drm_modeset_lock_all_ctx(f->dev, &ctx);
		if (!ret)
			ret = operation(f);
		if (ret != -EDEADLK)
			break;
		drm_atomic_commit_clear(f->state);
		ret = drm_modeset_backoff(&ctx);
		if (ret)
			break;
	}
	drm_modeset_drop_locks(&ctx);
	f->state->acquire_ctx = NULL;
	drm_modeset_acquire_fini(&ctx);
	return ret;
}

static int set_position(struct cursor_fixture *f)
{
	return drm_atomic_set_legacy_cursor_position(f->state, f->crtc, -17, 29);
}

static void pending_position_does_not_change_current_position(struct kunit *test)
{
	struct cursor_fixture *f = new_fixture(test);
	struct __drm_crtcs_state *entry = &f->state->crtcs[drm_crtc_index(f->crtc)];

	KUNIT_ASSERT_EQ(test, run_update(f, set_position), 0);
	KUNIT_EXPECT_EQ(test, f->crtc->cursor_x, 3);
	KUNIT_EXPECT_EQ(test, f->crtc->cursor_y, 5);
	KUNIT_EXPECT_TRUE(test, entry->update_cursor_position);
	KUNIT_EXPECT_EQ(test, entry->cursor_x, -17);
	KUNIT_EXPECT_EQ(test, entry->cursor_y, 29);
}

static int accept_position(struct cursor_fixture *f)
{
	int ret = set_position(f);

	if (!ret)
		ret = drm_atomic_check_only(f->state);
	if (!ret)
		ret = drm_atomic_helper_swap_state(f->state, false);
	return ret;
}

static void accepted_position_changes_with_state(struct kunit *test)
{
	struct cursor_fixture *f = new_fixture(test);

	KUNIT_ASSERT_EQ(test, run_update(f, accept_position), 0);
	KUNIT_EXPECT_EQ(test, f->crtc->cursor_x, -17);
	KUNIT_EXPECT_EQ(test, f->crtc->cursor_y, 29);
}

static int accept_without_position(struct cursor_fixture *f)
{
	struct drm_crtc_state *crtc_state = drm_atomic_get_crtc_state(f->state, f->crtc);
	int ret;

	if (IS_ERR(crtc_state))
		return PTR_ERR(crtc_state);
	ret = drm_atomic_check_only(f->state);
	if (!ret)
		ret = drm_atomic_helper_swap_state(f->state, false);
	return ret;
}

static void clearing_discards_pending_position(struct kunit *test)
{
	struct cursor_fixture *f = new_fixture(test);

	KUNIT_ASSERT_EQ(test, run_update(f, set_position), 0);
	drm_atomic_commit_clear(f->state);
	KUNIT_EXPECT_FALSE(test, f->state->crtcs[drm_crtc_index(f->crtc)].update_cursor_position);
	KUNIT_ASSERT_EQ(test, run_update(f, accept_without_position), 0);
	KUNIT_EXPECT_EQ(test, f->crtc->cursor_x, 3);
	KUNIT_EXPECT_EQ(test, f->crtc->cursor_y, 5);
}

static struct kunit_case cases[] = {
	KUNIT_CASE(pending_position_does_not_change_current_position),
	KUNIT_CASE(accepted_position_changes_with_state),
	KUNIT_CASE(clearing_discards_pending_position),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_cursor",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_LICENSE("GPL");
