// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/module.h>
#include <drm/drm_atomic.h>
#include <drm/drm_atomic_constraints.h>
#include <drm/drm_atomic_helper.h>
#include <drm/drm_atomic_uapi.h>
#include <drm/drm_constraints.h>
#include <drm/drm_constraints_catalog.h>
#include <drm/drm_constraints_device.h>
#include <drm/drm_constraints_entry.h>
#include <drm/drm_constraints_output.h>
#include <drm/drm_fourcc.h>
#include <drm/drm_framebuffer.h>
#include <drm/drm_kunit_helpers.h>
#include <drm/drm_modeset_helper.h>
#include <kunit/test.h>

#include "../drm_crtc_internal.h"

/* Metadata-only provider: framebuffer creation performs no GPU allocation. */
struct test_backend {
	bool failed;
	unsigned int checks;
};

struct atomic_fixture {
	struct drm_device *dev;
	struct drm_crtc *crtc;
	struct drm_plane *plane;
	struct drm_constraints_entry *initial;
	struct drm_constraints_entry *target;
	struct test_backend backends[2];
	struct drm_framebuffer *linear;
	struct drm_framebuffer *tiled;
	unsigned int installs;
};

static void destroy_fb(struct drm_framebuffer *fb)
{
	drm_framebuffer_cleanup(fb);
	kfree(fb);
}

static const struct drm_framebuffer_funcs fb_ops = { .destroy = destroy_fb };

static struct drm_framebuffer *
create_fb(struct drm_device *dev, struct drm_file *file,
	  const struct drm_format_info *info, const struct drm_mode_fb_cmd2 *cmd)
{
	struct drm_framebuffer *fb;
	int ret;

	if (!drm_any_plane_has_format(dev, cmd->pixel_format, cmd->modifier[0]))
		return ERR_PTR(-EINVAL);
	fb = kzalloc_obj(*fb);
	if (!fb)
		return ERR_PTR(-ENOMEM);
	drm_helper_mode_fill_fb_struct(dev, fb, info, cmd);
	ret = drm_framebuffer_init(dev, fb, &fb_ops);
	if (ret) {
		kfree(fb);
		return ERR_PTR(ret);
	}
	return fb;
}

static const struct drm_mode_config_funcs mode_ops = { .fb_create = create_fb };

static void release_backend(void *data) { }
static const struct drm_constraints_entry_ops entry_ops = {
	.owner = THIS_MODULE,
	.release = release_backend,
};

static int check_backend(struct drm_atomic_commit *state,
			 const struct drm_crtc_state *crtc, void *data)
{
	struct test_backend *backend = data;

	backend->checks++;
	return backend->failed ? -EIO : 0;
}

static const struct drm_constraints_output_ops output_ops = { .check = check_backend };

static void put_description(void *data) { drm_constraints_description_put(data); }
static void put_entry(void *data) { drm_constraints_entry_put(data); }
static void put_fb(void *data) { drm_framebuffer_put(data); }

static struct drm_constraints_entry *
new_entry(struct kunit *test, struct atomic_fixture *f, u32 format, u64 modifier, unsigned int backend)
{
	const struct drm_constraints_size size = { 128, 64, 128, 64 };
	const struct drm_constraints_format allocation = {
		.plane_id = f->plane->base.id,
		.format = format,
		.modifier = modifier,
		.size = size,
	};
	struct drm_constraints_description *description;
	struct drm_constraints_entry *entry;

	description = drm_constraints_description_create(&size, &allocation, 1);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, description);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_description, description), 0);
	entry = drm_constraints_entry_create(drm_constraints_device_domain(f->dev), f->crtc->base.id,
					     description, &entry_ops, &f->backends[backend]);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, entry);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_entry, entry), 0);
	return entry;
}

static struct drm_framebuffer *
new_fb(struct kunit *test, struct atomic_fixture *f, u32 format, u64 modifier, u32 width)
{
	struct drm_mode_fb_cmd2 cmd = {
		.width = width, .height = 64, .pixel_format = format,
		.flags = DRM_MODE_FB_MODIFIERS,
		.handles = { 1 }, .pitches = { width * 4 }, .modifier = { modifier },
	};
	struct drm_framebuffer *fb = drm_internal_framebuffer_create(f->dev, &cmd, NULL);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, fb);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_fb, fb), 0);
	return fb;
}

static struct atomic_fixture *new_fixture(struct kunit *test)
{
	static const u32 formats[] = { DRM_FORMAT_XRGB8888, DRM_FORMAT_ARGB8888 };
	static const u64 modifiers[] = {
		DRM_FORMAT_MOD_LINEAR, I915_FORMAT_MOD_X_TILED, DRM_FORMAT_MOD_INVALID,
	};
	struct atomic_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct device *parent;

	KUNIT_ASSERT_NOT_NULL(test, f);
	parent = drm_kunit_helper_alloc_device(test);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	f->dev = __drm_kunit_helper_alloc_drm_device(test, parent, sizeof(*f->dev), 0,
						    DRIVER_MODESET | DRIVER_ATOMIC);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->dev);
	f->dev->dev_private = f;
	f->dev->mode_config.funcs = &mode_ops;
	f->dev->mode_config.min_width = f->dev->mode_config.min_height = 1;
	f->dev->mode_config.max_width = f->dev->mode_config.max_height = 1024;
	KUNIT_ASSERT_EQ(test, drm_constraints_device_init(f->dev, 8), 0);
	f->plane = drm_kunit_helper_create_primary_plane(test, f->dev, NULL, NULL,
							 formats, ARRAY_SIZE(formats), modifiers);
	f->crtc = drm_kunit_helper_create_crtc(test, f->dev, f->plane, NULL, NULL, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->crtc);
	drm_mode_config_reset(f->dev);
	f->initial = new_entry(test, f, DRM_FORMAT_XRGB8888, DRM_FORMAT_MOD_LINEAR, 0);
	f->target = new_entry(test, f, DRM_FORMAT_ARGB8888, I915_FORMAT_MOD_X_TILED, 1);
	KUNIT_ASSERT_EQ(test, drm_constraints_crtc_init(f->crtc, f->initial, 4, &output_ops), 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_crtc_add(f->crtc, f->target), 0);
	f->linear = new_fb(test, f, DRM_FORMAT_XRGB8888, DRM_FORMAT_MOD_LINEAR, 128);
	f->tiled = new_fb(test, f, DRM_FORMAT_ARGB8888, I915_FORMAT_MOD_X_TILED, 128);
	return f;
}

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

static struct drm_atomic_commit *
new_update(struct kunit *test, struct atomic_fixture *f,
	   struct drm_constraints_entry *entry, struct drm_framebuffer *fb)
{
	const struct drm_display_mode mode = {
		DRM_MODE("128x64", 0, 1000, 128, 132, 136, 144, 0, 64, 66, 68, 72, 0, 0),
	};
	struct drm_atomic_commit *state = drm_kunit_helper_atomic_state_alloc(test, f->dev, NULL);
	struct drm_modeset_acquire_ctx ctx;
	struct drm_crtc_state *crtc;
	struct drm_plane_state *plane;
	int ret;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	ret = lock_update(state, &ctx);
	if (ret)
		goto out;
	crtc = drm_atomic_get_crtc_state(state, f->crtc);
	if (IS_ERR(crtc)) {
		ret = PTR_ERR(crtc);
		goto out;
	}
	if (entry) {
		ret = drm_atomic_set_constraints_for_crtc(crtc, entry);
		if (ret)
			goto out;
	}
	ret = drm_atomic_set_mode_for_crtc(crtc, &mode);
	if (ret)
		goto out;
	crtc->active = true;
	plane = drm_atomic_get_plane_state(state, f->plane);
	if (IS_ERR(plane)) {
		ret = PTR_ERR(plane);
		goto out;
	}
	ret = drm_atomic_set_crtc_for_plane(plane, f->crtc);
	if (ret)
		goto out;
	drm_atomic_set_fb_for_plane(plane, fb);
	plane->src_w = fb->width << 16;
	plane->src_h = fb->height << 16;
	plane->crtc_w = 128;
	plane->crtc_h = 64;
out:
	unlock_update(state);
	return ret ? ERR_PTR(ret) : state;
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

static int check_update(struct drm_atomic_commit *state)
{
	int ret = drm_atomic_constraints_prepare(state);

	return ret ? ret : drm_atomic_constraints_check(state);
}

static void record_install(struct drm_atomic_commit *state)
{
	struct atomic_fixture *f = state->dev->dev_private;

	f->installs++;
}

static int install_update(struct drm_atomic_commit *state)
{
	return drm_atomic_constraints_install(state, record_install);
}

static void target_creation_precedes_atomic_selection(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_atomic_commit *state = new_update(test, f, NULL, f->tiled);
	struct drm_constraints_entry *selected;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	KUNIT_EXPECT_EQ(test, run_update(state, check_update), -EINVAL);
	drm_atomic_commit_clear(state);
	state = new_update(test, f, f->target, f->tiled);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	KUNIT_ASSERT_EQ(test, run_update(state, check_update), 0);
	selected = drm_constraints_catalog_selected(drm_constraints_crtc_catalog(f->crtc));
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_entry, selected), 0);
	KUNIT_EXPECT_PTR_EQ(test, selected, f->initial);
	KUNIT_EXPECT_EQ(test, f->backends[0].checks, 0);
	KUNIT_EXPECT_EQ(test, f->backends[1].checks, 1);
}

static void readiness_loss_after_check_prevents_installation(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_atomic_commit *state = new_update(test, f, f->target, f->tiled);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	KUNIT_ASSERT_EQ(test, run_update(state, check_update), 0);
	f->backends[1].failed = true;
	KUNIT_EXPECT_EQ(test, run_update(state, install_update), -EIO);
	KUNIT_EXPECT_EQ(test, f->installs, 0);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, f->initial);
}

static void withdrawal_after_check_prevents_installation(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_atomic_commit *state = new_update(test, f, f->target, f->tiled);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	KUNIT_ASSERT_EQ(test, run_update(state, check_update), 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_catalog_withdraw(drm_constraints_crtc_catalog(f->crtc),
							      drm_constraints_entry_id(f->target)), 0);
	KUNIT_EXPECT_EQ(test, run_update(state, install_update), -ESTALE);
	KUNIT_EXPECT_EQ(test, f->installs, 0);
}

static void selection_requires_modeset_permission(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_atomic_commit *state = new_update(test, f, f->target, f->tiled);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	state->allow_modeset = false;
	KUNIT_EXPECT_EQ(test, run_update(state, check_update), -EINVAL);
	KUNIT_EXPECT_EQ(test, run_update(state, install_update), -EINVAL);
	KUNIT_EXPECT_EQ(test, f->installs, 0);
}

static void source_allocation_respects_exact_geometry(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_framebuffer *small = new_fb(test, f, DRM_FORMAT_XRGB8888, DRM_FORMAT_MOD_LINEAR, 64);
	struct drm_atomic_commit *state = new_update(test, f, NULL, small);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	KUNIT_EXPECT_EQ(test, run_update(state, check_update), -EINVAL);
}

static void asynchronous_updates_are_not_admitted(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_atomic_commit *state = new_update(test, f, NULL, f->linear);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	state->async_update = true;
	KUNIT_EXPECT_EQ(test, run_update(state, check_update), -EOPNOTSUPP);
	KUNIT_EXPECT_EQ(test, run_update(state, install_update), -EOPNOTSUPP);
	KUNIT_EXPECT_EQ(test, f->installs, 0);
}

static void multi_output_transactions_are_not_admitted(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_plane *plane = drm_kunit_helper_create_primary_plane(test, f->dev,
							NULL, NULL, NULL, 0, NULL);
	struct drm_crtc *other = drm_kunit_helper_create_crtc(test, f->dev, plane, NULL, NULL, NULL);
	struct drm_atomic_commit *state;
	struct drm_crtc_state *added;
	struct drm_modeset_acquire_ctx ctx;
	int ret;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, other);
	drm_mode_config_reset(f->dev);
	state = new_update(test, f, f->target, f->tiled);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	ret = lock_update(state, &ctx);
	added = ret ? ERR_PTR(ret) : drm_atomic_get_crtc_state(state, other);
	unlock_update(state);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, added);
	KUNIT_EXPECT_EQ(test, run_update(state, check_update), -EOPNOTSUPP);
	KUNIT_EXPECT_EQ(test, run_update(state, install_update), -EOPNOTSUPP);
	KUNIT_EXPECT_EQ(test, f->installs, 0);
}

static int swap_update(struct drm_atomic_commit *state)
{
	return drm_atomic_helper_swap_state(state, false);
}

static void validation_includes_unchanged_active_planes(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_atomic_commit *first = new_update(test, f, NULL, f->linear);
	struct drm_atomic_commit *next;
	struct drm_crtc_state *crtc;
	struct drm_modeset_acquire_ctx ctx;
	int ret;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, first);
	KUNIT_ASSERT_EQ(test, run_update(first, check_update), 0);
	KUNIT_ASSERT_EQ(test, run_update(first, swap_update), 0);
	next = drm_kunit_helper_atomic_state_alloc(test, f->dev, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, next);
	ret = lock_update(next, &ctx);
	crtc = ret ? ERR_PTR(ret) : drm_atomic_get_crtc_state(next, f->crtc);
	if (!IS_ERR(crtc))
		ret = drm_atomic_set_constraints_for_crtc(crtc, f->target);
	else
		ret = PTR_ERR(crtc);
	unlock_update(next);
	KUNIT_ASSERT_EQ(test, ret, 0);
	KUNIT_EXPECT_PTR_EQ(test, drm_atomic_get_new_plane_state(next, f->plane), NULL);
	KUNIT_EXPECT_EQ(test, run_update(next, check_update), -EINVAL);
	KUNIT_EXPECT_NOT_NULL(test, drm_atomic_get_new_plane_state(next, f->plane));
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, f->initial);
}

static void core_validation_observes_selected_constraints(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_atomic_commit *state = new_update(test, f, NULL, f->tiled);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	KUNIT_EXPECT_EQ(test, run_update(state, drm_atomic_check_only), -EINVAL);
	KUNIT_EXPECT_FALSE(test, state->checked);
	drm_atomic_commit_clear(state);
	state = new_update(test, f, f->target, f->tiled);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	KUNIT_ASSERT_EQ(test, run_update(state, drm_atomic_check_only), 0);
	KUNIT_EXPECT_TRUE(test, state->checked);
	KUNIT_EXPECT_TRUE(test, drm_atomic_get_new_crtc_state(state, f->crtc)->mode_changed);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, f->initial);
}

static void state_swap_accepts_retained_backend(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_atomic_commit *first = new_update(test, f, NULL, f->linear);
	struct drm_atomic_commit *next;
	struct drm_constraints_entry *selected;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, first);
	KUNIT_ASSERT_EQ(test, run_update(first, drm_atomic_check_only), 0);
	KUNIT_ASSERT_EQ(test, run_update(first, swap_update), 0);
	next = new_update(test, f, f->target, f->tiled);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, next);
	KUNIT_ASSERT_EQ(test, run_update(next, drm_atomic_check_only), 0);
	KUNIT_ASSERT_EQ(test, run_update(next, swap_update), 0);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, f->target);
	KUNIT_EXPECT_PTR_EQ(test, f->plane->state->fb, f->tiled);
	KUNIT_EXPECT_PTR_EQ(test, drm_atomic_get_old_crtc_state(next, f->crtc)->constraints,
			    f->initial);
	KUNIT_EXPECT_PTR_EQ(test, drm_atomic_get_new_crtc_state(first, f->crtc)->constraints,
			    f->initial);
	selected = drm_constraints_catalog_selected(drm_constraints_crtc_catalog(f->crtc));
	KUNIT_EXPECT_PTR_EQ(test, selected, f->target);
	drm_constraints_entry_put(selected);
}

static void state_swap_rechecks_withdrawn_target(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_constraints_catalog *catalog = drm_constraints_crtc_catalog(f->crtc);
	struct drm_atomic_commit *state = new_update(test, f, f->target, f->tiled);
	struct drm_crtc_state *before = f->crtc->state;
	struct drm_plane_state *plane_before = f->plane->state;
	struct drm_constraints_entry *selected;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	KUNIT_ASSERT_EQ(test, run_update(state, drm_atomic_check_only), 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_catalog_withdraw(catalog,
						      drm_constraints_entry_id(f->target)), 0);
	KUNIT_EXPECT_EQ(test, run_update(state, swap_update), -ESTALE);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state, before);
	KUNIT_EXPECT_PTR_EQ(test, f->plane->state, plane_before);
	selected = drm_constraints_catalog_selected(drm_constraints_crtc_catalog(f->crtc));
	KUNIT_EXPECT_PTR_EQ(test, selected, f->initial);
	drm_constraints_entry_put(selected);
}

static void state_swap_rechecks_failed_backend(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_atomic_commit *state = new_update(test, f, f->target, f->tiled);
	struct drm_crtc_state *before = f->crtc->state;
	struct drm_plane_state *plane_before = f->plane->state;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	KUNIT_ASSERT_EQ(test, run_update(state, drm_atomic_check_only), 0);
	f->backends[1].failed = true;
	KUNIT_EXPECT_EQ(test, run_update(state, swap_update), -EIO);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state, before);
	KUNIT_EXPECT_PTR_EQ(test, f->plane->state, plane_before);
}

static void accepted_selection_persists_without_reselection(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_constraints_catalog *catalog = drm_constraints_crtc_catalog(f->crtc);
	struct drm_atomic_commit *first = new_update(test, f, f->target, f->tiled);
	struct drm_atomic_commit *next;
	struct drm_constraints_snapshot *snapshot;
	u64 generation;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, first);
	KUNIT_ASSERT_EQ(test, run_update(first, drm_atomic_check_only), 0);
	KUNIT_ASSERT_EQ(test, run_update(first, swap_update), 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_catalog_withdraw(catalog,
						      drm_constraints_entry_id(f->target)), 0);
	snapshot = drm_constraints_catalog_snapshot(catalog, 0);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, snapshot);
	generation = drm_constraints_snapshot_info(snapshot)->generation;
	drm_constraints_snapshot_put(snapshot);
	next = new_update(test, f, NULL, f->tiled);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, next);
	next->allow_modeset = false;
	KUNIT_ASSERT_EQ(test, run_update(next, drm_atomic_check_only), 0);
	KUNIT_ASSERT_EQ(test, run_update(next, swap_update), 0);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, f->target);
	snapshot = drm_constraints_catalog_snapshot(catalog, generation);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, snapshot);
	KUNIT_EXPECT_EQ(test, drm_constraints_snapshot_info(snapshot)->selected_id,
			drm_constraints_entry_id(f->target));
	drm_constraints_snapshot_put(snapshot);
}

static struct kunit_case drm_constraints_atomic_tests[] = {
	KUNIT_CASE(target_creation_precedes_atomic_selection),
	KUNIT_CASE(readiness_loss_after_check_prevents_installation),
	KUNIT_CASE(withdrawal_after_check_prevents_installation),
	KUNIT_CASE(selection_requires_modeset_permission),
	KUNIT_CASE(source_allocation_respects_exact_geometry),
	KUNIT_CASE(asynchronous_updates_are_not_admitted),
	KUNIT_CASE(multi_output_transactions_are_not_admitted),
	KUNIT_CASE(validation_includes_unchanged_active_planes),
	KUNIT_CASE(core_validation_observes_selected_constraints),
	KUNIT_CASE(state_swap_accepts_retained_backend),
	KUNIT_CASE(state_swap_rechecks_withdrawn_target),
	KUNIT_CASE(state_swap_rechecks_failed_backend),
	KUNIT_CASE(accepted_selection_persists_without_reselection),
	{}
};

static struct kunit_suite drm_constraints_atomic_test_suite = {
	.name = "drm_constraints_atomic",
	.test_cases = drm_constraints_atomic_tests,
};

kunit_test_suite(drm_constraints_atomic_test_suite);

MODULE_LICENSE("Dual MIT/GPL");
