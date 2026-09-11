// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <drm/drm_atomic.h>
#include <drm/drm_file.h>
#include <drm/drm_kunit_helpers.h>
#include <drm/drm_modeset_lock.h>
#include <drm/drm_vblank.h>
#include <kunit/test.h>

#include "../drm_atomic_user_signaling.h"

struct signaling_fixture {
	struct drm_device *dev;
	struct drm_crtc *crtc;
	struct drm_file file;
	struct drm_pending_vblank_event foreign_event;
	bool had_signaling;
	bool had_event;
	bool kept_foreign_event;
	bool event_remained_after_completion;
	u64 event_user_data;
};

static struct signaling_fixture *new_fixture(struct kunit *test)
{
	struct signaling_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
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
	INIT_LIST_HEAD(&f->file.pending_event_list);
	f->file.event_space = 4096;
	return f;
}

/* Finish metadata, state and locks before any assertion can abort the test. */
static int run_attempt(struct signaling_fixture *f, u32 flags, bool include_crtc,
		       bool active, bool foreign_event, bool accepted)
{
	struct drm_atomic_commit *state = drm_atomic_commit_alloc(f->dev);
	struct drm_atomic_user_signaling *signaling = NULL;
	struct drm_modeset_acquire_ctx ctx;
	struct drm_crtc_state *crtc_state = NULL;
	int ret;

	if (!state)
		return -ENOMEM;
	drm_modeset_acquire_init(&ctx, 0);
	state->acquire_ctx = &ctx;
	for (;;) {
		ret = drm_modeset_lock_all_ctx(f->dev, &ctx);
		if (ret != -EDEADLK)
			break;
		ret = drm_modeset_backoff(&ctx);
		if (ret)
			break;
	}
	if (ret)
		goto out;
	if (include_crtc) {
		crtc_state = drm_atomic_get_crtc_state(state, f->crtc);
		if (IS_ERR(crtc_state)) {
			ret = PTR_ERR(crtc_state);
			goto out;
		}
		crtc_state->active = active;
		if (foreign_event)
			crtc_state->event = &f->foreign_event;
	}
	ret = drm_atomic_prepare_user_signaling(state, &f->file, flags, 0x123456789ULL,
						&signaling);
	f->had_signaling = !!signaling;
	if (crtc_state) {
		f->had_event = !!crtc_state->event;
		if (crtc_state->event)
			f->event_user_data = crtc_state->event->event.vbl.user_data;
	}
	drm_atomic_complete_user_signaling(state, signaling, accepted && !ret);
	if (crtc_state) {
		f->event_remained_after_completion = !!crtc_state->event;
		f->kept_foreign_event = crtc_state->event == &f->foreign_event;
		if (crtc_state->event && !f->kept_foreign_event)
			drm_event_cancel_free(f->dev, &crtc_state->event->base);
		crtc_state->event = NULL;
	}
out:
	drm_atomic_commit_put(state);
	drm_modeset_drop_locks(&ctx);
	drm_modeset_acquire_fini(&ctx);
	return ret;
}

static void rejected_commit_returns_event_space(struct kunit *test)
{
	struct signaling_fixture *f = new_fixture(test);

	KUNIT_ASSERT_EQ(test, run_attempt(f, DRM_MODE_PAGE_FLIP_EVENT, true, true, false, false), 0);
	KUNIT_EXPECT_TRUE(test, f->had_event);
	KUNIT_EXPECT_FALSE(test, f->event_remained_after_completion);
	KUNIT_EXPECT_EQ(test, f->event_user_data, 0x123456789ULL);
	KUNIT_EXPECT_EQ(test, f->file.event_space, 4096);
	KUNIT_EXPECT_TRUE(test, list_empty(&f->file.pending_event_list));
}

static void test_only_allocates_no_signaling(struct kunit *test)
{
	struct signaling_fixture *f = new_fixture(test);

	KUNIT_ASSERT_EQ(test, run_attempt(f, DRM_MODE_ATOMIC_TEST_ONLY, true, true, false, false), 0);
	KUNIT_EXPECT_FALSE(test, f->had_signaling);
	KUNIT_EXPECT_FALSE(test, f->had_event);
}

static void event_requires_a_controller(struct kunit *test)
{
	struct signaling_fixture *f = new_fixture(test);

	KUNIT_EXPECT_EQ(test, run_attempt(f, DRM_MODE_PAGE_FLIP_EVENT, false, false, false, false),
			-EINVAL);
	KUNIT_EXPECT_EQ(test, f->file.event_space, 4096);
}

static void inactive_controller_rejects_event(struct kunit *test)
{
	struct signaling_fixture *f = new_fixture(test);

	KUNIT_EXPECT_EQ(test, run_attempt(f, DRM_MODE_PAGE_FLIP_EVENT, true, false, false, false),
			-EINVAL);
	KUNIT_EXPECT_FALSE(test, f->had_event);
	KUNIT_EXPECT_EQ(test, f->file.event_space, 4096);
}

static void reservation_failure_releases_unreserved_event(struct kunit *test)
{
	struct signaling_fixture *f = new_fixture(test);

	f->file.event_space = 0;
	KUNIT_EXPECT_EQ(test, run_attempt(f, DRM_MODE_PAGE_FLIP_EVENT, true, true, false, false),
			-ENOMEM);
	KUNIT_EXPECT_TRUE(test, f->had_event);
	KUNIT_EXPECT_FALSE(test, f->event_remained_after_completion);
	KUNIT_EXPECT_EQ(test, f->file.event_space, 0);
	KUNIT_EXPECT_TRUE(test, list_empty(&f->file.pending_event_list));
}

static struct kunit_case cases[] = {
	KUNIT_CASE(rejected_commit_returns_event_space),
	KUNIT_CASE(test_only_allocates_no_signaling),
	KUNIT_CASE(event_requires_a_controller),
	KUNIT_CASE(inactive_controller_rejects_event),
	KUNIT_CASE(reservation_failure_releases_unreserved_event),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_user_signaling",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_IMPORT_NS("EXPORTED_FOR_KUNIT_TESTING");
MODULE_LICENSE("GPL");
