// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <drm/drm_atomic.h>
#include <drm/drm_atomic_gamma.h>
#include <drm/drm_atomic_helper.h>
#include <drm/drm_atomic_prepare_display.h>
#include <drm/drm_color_mgmt.h>
#include <drm/drm_kunit_helpers.h>
#include <drm/drm_property.h>
#include <kunit/test.h>

struct gamma_fixture {
	struct drm_device *dev;
	struct drm_crtc *crtc;
	struct drm_atomic_commit *state;
	struct drm_property_blob *table;
};

static void finish_fixture(void *data)
{
	struct gamma_fixture *f = data;

	drm_atomic_commit_put(f->state);
	drm_property_blob_put(f->table);
}

static struct gamma_fixture *new_fixture(struct kunit *test, bool gamma)
{
	const struct drm_color_lut entries[] = {
		{ .red = 11, .green = 13, .blue = 17 },
		{ .red = 19, .green = 23, .blue = 29 },
	};
	struct gamma_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct device *parent = drm_kunit_helper_alloc_device(test);
	struct drm_plane *plane;

	KUNIT_ASSERT_NOT_NULL(test, f);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	f->dev = __drm_kunit_helper_alloc_drm_device(test, parent, sizeof(*f->dev), 0,
						  DRIVER_MODESET | DRIVER_ATOMIC);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->dev);
	KUNIT_ASSERT_EQ(test, drm_atomic_prepare_display_init(f->dev, 8), 0);
	plane = drm_kunit_helper_create_primary_plane(test, f->dev, NULL, NULL, NULL, 0, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, plane);
	f->crtc = drm_kunit_helper_create_crtc(test, f->dev, plane, NULL, NULL, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->crtc);
	drm_crtc_enable_color_mgmt(f->crtc, 2, true, gamma ? 2 : 0);
	KUNIT_ASSERT_EQ(test, drm_mode_crtc_set_gamma_size(f->crtc, 2), 0);
	drm_mode_config_reset(f->dev);
	f->state = drm_atomic_commit_alloc(f->dev);
	KUNIT_ASSERT_NOT_NULL(test, f->state);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, finish_fixture, f), 0);
	f->table = drm_property_create_blob(f->dev, sizeof(entries), entries);
	if (IS_ERR(f->table)) {
		f->table = NULL;
		KUNIT_FAIL(test, "Cannot create gamma table");
	}
	return f;
}

static int run_update(struct gamma_fixture *f, int (*operation)(struct gamma_fixture *))
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

static int set_table(struct gamma_fixture *f)
{
	return drm_atomic_set_legacy_gamma(f->state, f->crtc, f->table);
}

static void pending_table_does_not_change_readback(struct kunit *test)
{
	struct gamma_fixture *f = new_fixture(test, true);
	u16 before[6];
	struct drm_crtc_state *new;

	memcpy(before, f->crtc->gamma_store, sizeof(before));
	KUNIT_ASSERT_EQ(test, run_update(f, set_table), 0);
	KUNIT_EXPECT_MEMEQ(test, before, f->crtc->gamma_store, sizeof(before));
	new = drm_atomic_get_new_crtc_state(f->state, f->crtc);
	KUNIT_EXPECT_PTR_EQ(test, new->gamma_lut, f->table);
	KUNIT_EXPECT_PTR_EQ(test, f->state->crtcs[drm_crtc_index(f->crtc)].legacy_gamma, f->table);
	KUNIT_EXPECT_TRUE(test, new->color_mgmt_changed);
}

static int accept_table(struct gamma_fixture *f)
{
	int ret = set_table(f);

	if (!ret)
		ret = drm_atomic_check_only(f->state);
	if (!ret)
		ret = drm_atomic_helper_swap_state(f->state, false);
	return ret;
}

static void accepted_table_changes_readback(struct kunit *test)
{
	const u16 expected[] = { 11, 19, 13, 23, 17, 29 };
	struct gamma_fixture *f = new_fixture(test, true);

	KUNIT_ASSERT_EQ(test, run_update(f, accept_table), 0);
	KUNIT_EXPECT_MEMEQ(test, expected, f->crtc->gamma_store, sizeof(expected));
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->gamma_lut, f->table);
}

static void clearing_discards_pending_table(struct kunit *test)
{
	struct gamma_fixture *f = new_fixture(test, true);
	u16 before[6];

	memcpy(before, f->crtc->gamma_store, sizeof(before));
	KUNIT_ASSERT_EQ(test, run_update(f, set_table), 0);
	drm_atomic_commit_clear(f->state);
	KUNIT_EXPECT_NULL(test, f->state->crtcs[drm_crtc_index(f->crtc)].legacy_gamma);
	KUNIT_EXPECT_MEMEQ(test, before, f->crtc->gamma_store, sizeof(before));
}

static int reject_table(struct gamma_fixture *f)
{
	int ret = set_table(f);

	if (ret)
		return ret;
	drm_atomic_get_new_crtc_state(f->state, f->crtc)->active = true;
	return drm_atomic_check_only(f->state);
}

static void failed_check_preserves_readback(struct kunit *test)
{
	struct gamma_fixture *f = new_fixture(test, true);
	u16 before[6];

	memcpy(before, f->crtc->gamma_store, sizeof(before));
	KUNIT_EXPECT_EQ(test, run_update(f, reject_table), -EINVAL);
	KUNIT_EXPECT_MEMEQ(test, before, f->crtc->gamma_store, sizeof(before));
}

static int change_checked_table(struct gamma_fixture *f)
{
	int ret = set_table(f);

	if (!ret)
		ret = drm_atomic_check_only(f->state);
	if (!ret)
		ret = set_table(f);
	return ret;
}

static void checked_table_cannot_be_replaced(struct kunit *test)
{
	struct gamma_fixture *f = new_fixture(test, true);

	KUNIT_EXPECT_EQ(test, run_update(f, change_checked_table), -EINVAL);
}

static void degamma_property_accepts_legacy_table(struct kunit *test)
{
	const u16 expected[] = { 11, 19, 13, 23, 17, 29 };
	struct gamma_fixture *f = new_fixture(test, false);

	KUNIT_ASSERT_EQ(test, run_update(f, accept_table), 0);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->degamma_lut, f->table);
	KUNIT_EXPECT_NULL(test, f->crtc->state->gamma_lut);
	KUNIT_EXPECT_MEMEQ(test, expected, f->crtc->gamma_store, sizeof(expected));
}

static void short_table_is_rejected_before_assignment(struct kunit *test)
{
	struct gamma_fixture *f = new_fixture(test, true);
	struct drm_property_blob *short_table;

	short_table = drm_property_create_blob(f->dev, sizeof(struct drm_color_lut), NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, short_table);
	drm_property_blob_put(f->table);
	f->table = short_table;
	KUNIT_EXPECT_EQ(test, run_update(f, set_table), -EINVAL);
	KUNIT_EXPECT_NULL(test, drm_atomic_get_new_crtc_state(f->state, f->crtc));
	KUNIT_EXPECT_NULL(test, f->state->crtcs[drm_crtc_index(f->crtc)].legacy_gamma);
}

static int set_with_other_colors(struct gamma_fixture *f)
{
	struct drm_crtc_state *new = drm_atomic_get_crtc_state(f->state, f->crtc);
	struct drm_property_blob *matrix;

	if (IS_ERR(new))
		return PTR_ERR(new);
	matrix = drm_property_create_blob(f->dev, sizeof(struct drm_color_ctm), NULL);
	if (IS_ERR(matrix))
		return PTR_ERR(matrix);
	drm_property_replace_blob(&new->ctm, matrix);
	drm_property_blob_put(matrix);
	drm_property_replace_blob(&new->degamma_lut, f->table);
	return set_table(f);
}

static void legacy_table_clears_other_color_stages(struct kunit *test)
{
	struct gamma_fixture *f = new_fixture(test, true);
	struct drm_crtc_state *new;

	KUNIT_ASSERT_EQ(test, run_update(f, set_with_other_colors), 0);
	new = drm_atomic_get_new_crtc_state(f->state, f->crtc);
	KUNIT_EXPECT_PTR_EQ(test, new->gamma_lut, f->table);
	KUNIT_EXPECT_NULL(test, new->degamma_lut);
	KUNIT_EXPECT_NULL(test, new->ctm);
}

static struct kunit_case cases[] = {
	KUNIT_CASE(pending_table_does_not_change_readback),
	KUNIT_CASE(accepted_table_changes_readback),
	KUNIT_CASE(clearing_discards_pending_table),
	KUNIT_CASE(failed_check_preserves_readback),
	KUNIT_CASE(checked_table_cannot_be_replaced),
	KUNIT_CASE(degamma_property_accepts_legacy_table),
	KUNIT_CASE(short_table_is_rejected_before_assignment),
	KUNIT_CASE(legacy_table_clears_other_color_stages),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_gamma",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_LICENSE("GPL");
