// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <drm/drm_atomic.h>
#include <drm/drm_atomic_helper.h>
#include <drm/drm_atomic_power.h>
#include <drm/drm_atomic_prepare_commit.h>
#include <drm/drm_atomic_prepare_display.h>
#include <drm/drm_atomic_prepare_outputs.h>
#include <drm/drm_atomic_prepare_ticket.h>
#include <drm/drm_atomic_uapi.h>
#include <drm/drm_connector.h>
#include <drm/drm_kunit_helpers.h>
#include <kunit/test.h>

struct power_fixture {
	struct drm_device *dev;
	struct drm_crtc *crtc;
	struct drm_connector connectors[2];
	struct drm_atomic_commit *state;
	bool reject;
};

static int check_update(struct drm_device *dev, struct drm_atomic_commit *state)
{
	struct power_fixture *f = dev->dev_private;

	return f->reject ? -EINVAL : 0;
}

static const struct drm_mode_config_funcs config_funcs = { .atomic_check = check_update };

static const struct drm_connector_funcs connector_funcs = {
	.reset = drm_atomic_helper_connector_reset,
	.atomic_duplicate_state = drm_atomic_helper_connector_duplicate_state,
	.atomic_destroy_state = drm_atomic_helper_connector_destroy_state,
};

static void finish_fixture(void *data)
{
	struct power_fixture *f = data;

	drm_atomic_commit_put(f->state);
}

static struct power_fixture *new_fixture(struct kunit *test)
{
	const struct drm_display_mode mode = {
		DRM_MODE("64x64", 0, 1000, 64, 65, 66, 67, 0, 64, 65, 66, 67, 0, 0)
	};
	struct power_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct device *parent = drm_kunit_helper_alloc_device(test);
	struct drm_plane *primary;
	int i, ret;

	KUNIT_ASSERT_NOT_NULL(test, f);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	f->dev = __drm_kunit_helper_alloc_drm_device(test, parent, sizeof(*f->dev), 0,
						  DRIVER_MODESET | DRIVER_ATOMIC);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->dev);
	f->dev->dev_private = f;
	f->dev->mode_config.funcs = &config_funcs;
	KUNIT_ASSERT_EQ(test, drm_atomic_prepare_display_init(f->dev, 8), 0);
	primary = drm_kunit_helper_create_primary_plane(test, f->dev, NULL, NULL, NULL, 0, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, primary);
	f->crtc = drm_kunit_helper_create_crtc(test, f->dev, primary, NULL, NULL, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->crtc);
	for (i = 0; i < ARRAY_SIZE(f->connectors); i++)
		KUNIT_ASSERT_EQ(test, drmm_connector_init(f->dev, &f->connectors[i],
				&connector_funcs, DRM_MODE_CONNECTOR_VIRTUAL, NULL), 0);
	drm_mode_config_reset(f->dev);
	ret = drm_modeset_lock(&f->crtc->mutex, NULL);
	KUNIT_ASSERT_EQ(test, ret, 0);
	ret = drm_atomic_set_mode_for_crtc(f->crtc->state, &mode);
	f->crtc->state->active = true;
	for (i = 0; i < ARRAY_SIZE(f->connectors); i++) {
		struct drm_connector *connector = &f->connectors[i];

		connector->dpms = DRM_MODE_DPMS_ON;
		drm_connector_get(connector);
		connector->state->crtc = f->crtc;
		f->crtc->state->connector_mask |= drm_connector_mask(connector);
	}
	drm_modeset_unlock(&f->crtc->mutex);
	KUNIT_ASSERT_EQ(test, ret, 0);
	f->state = drm_atomic_commit_alloc(f->dev);
	KUNIT_ASSERT_NOT_NULL(test, f->state);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, finish_fixture, f), 0);
	return f;
}

static int run_update(struct power_fixture *f, int (*operation)(struct power_fixture *))
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

static int first_off(struct power_fixture *f)
{
	return drm_atomic_set_connector_power(f->state, &f->connectors[0], false);
}

static void pending_preference_does_not_change_current_power(struct kunit *test)
{
	struct power_fixture *f = new_fixture(test);

	KUNIT_ASSERT_EQ(test, run_update(f, first_off), 0);
	KUNIT_EXPECT_EQ(test, f->connectors[0].dpms, DRM_MODE_DPMS_ON);
	KUNIT_EXPECT_TRUE(test, drm_atomic_get_new_crtc_state(f->state, f->crtc)->active);
}

static int both_off(struct power_fixture *f)
{
	int ret = first_off(f);

	if (!ret)
		ret = drm_atomic_set_connector_power(f->state, &f->connectors[1], false);
	return ret;
}

static void pending_preferences_combine_for_shared_controller(struct kunit *test)
{
	struct power_fixture *f = new_fixture(test);

	KUNIT_ASSERT_EQ(test, run_update(f, both_off), 0);
	KUNIT_EXPECT_FALSE(test, drm_atomic_get_new_crtc_state(f->state, f->crtc)->active);
	KUNIT_EXPECT_EQ(test, f->connectors[0].dpms, DRM_MODE_DPMS_ON);
	KUNIT_EXPECT_EQ(test, f->connectors[1].dpms, DRM_MODE_DPMS_ON);
}

static int accept_update(struct power_fixture *f)
{
	struct drm_prepare_output_generation entries[2];
	struct drm_prepare_ticket *ticket;
	int ret, count;

	ret = drm_atomic_check_only(f->state);
	if (ret)
		return ret;
	count = drm_atomic_prepare_display_observe(f->state, entries, ARRAY_SIZE(entries));
	if (count < 0)
		return count;
	ticket = drm_prepare_ticket_create(entries, count);
	if (IS_ERR(ticket))
		return PTR_ERR(ticket);
	ret = drm_atomic_commit_prepare(f->state, ticket, drm_atomic_prepare_display_observe);
	drm_prepare_ticket_put(ticket);
	if (!ret)
		ret = drm_atomic_helper_swap_state(f->state, false);
	return ret;
}

static int accept_first_off(struct power_fixture *f)
{
	int ret = first_off(f);

	return ret ?: accept_update(f);
}

static void acceptance_preserves_other_connector_preference(struct kunit *test)
{
	struct power_fixture *f = new_fixture(test);

	KUNIT_ASSERT_EQ(test, run_update(f, accept_first_off), 0);
	KUNIT_EXPECT_EQ(test, f->connectors[0].dpms, DRM_MODE_DPMS_OFF);
	KUNIT_EXPECT_EQ(test, f->connectors[1].dpms, DRM_MODE_DPMS_ON);
	KUNIT_EXPECT_TRUE(test, f->crtc->state->active);
}

static void rejected_update_preserves_power_preferences(struct kunit *test)
{
	struct power_fixture *f = new_fixture(test);

	f->reject = true;
	KUNIT_EXPECT_EQ(test, run_update(f, accept_first_off), -EINVAL);
	KUNIT_EXPECT_EQ(test, f->connectors[0].dpms, DRM_MODE_DPMS_ON);
	KUNIT_EXPECT_EQ(test, f->connectors[1].dpms, DRM_MODE_DPMS_ON);
}

static int include_first(struct power_fixture *f)
{
	struct drm_connector_state *state;

	state = drm_atomic_get_connector_state(f->state, &f->connectors[0]);
	return IS_ERR(state) ? PTR_ERR(state) : 0;
}

static void clearing_discards_power_preferences(struct kunit *test)
{
	struct power_fixture *f = new_fixture(test);
	unsigned int index = drm_connector_index(&f->connectors[0]);

	KUNIT_ASSERT_EQ(test, run_update(f, first_off), 0);
	drm_atomic_commit_clear(f->state);
	KUNIT_ASSERT_EQ(test, run_update(f, include_first), 0);
	KUNIT_EXPECT_FALSE(test, f->state->connectors[index].update_power);
	KUNIT_EXPECT_FALSE(test, f->state->crtcs[drm_crtc_index(f->crtc)].power_from_connectors);
	KUNIT_EXPECT_EQ(test, f->connectors[0].dpms, DRM_MODE_DPMS_ON);
}

static int change_checked_preference(struct power_fixture *f)
{
	int ret = first_off(f);

	if (!ret)
		ret = drm_atomic_check_only(f->state);
	if (!ret)
		ret = drm_atomic_set_connector_power(f->state, &f->connectors[0], true);
	return ret;
}

static void checked_preferences_cannot_be_replaced(struct kunit *test)
{
	struct power_fixture *f = new_fixture(test);
	unsigned int index = drm_connector_index(&f->connectors[0]);

	KUNIT_EXPECT_EQ(test, run_update(f, change_checked_preference), -EINVAL);
	KUNIT_EXPECT_TRUE(test, f->state->connectors[index].update_power);
	KUNIT_EXPECT_FALSE(test, f->state->connectors[index].power_on);
}

static struct kunit_case cases[] = {
	KUNIT_CASE(pending_preference_does_not_change_current_power),
	KUNIT_CASE(pending_preferences_combine_for_shared_controller),
	KUNIT_CASE(acceptance_preserves_other_connector_preference),
	KUNIT_CASE(rejected_update_preserves_power_preferences),
	KUNIT_CASE(clearing_discards_power_preferences),
	KUNIT_CASE(checked_preferences_cannot_be_replaced),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_power",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_LICENSE("GPL");
