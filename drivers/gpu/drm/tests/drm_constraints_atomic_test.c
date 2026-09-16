// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/completion.h>
#include <linux/dma-fence.h>
#include <linux/kthread.h>
#include <linux/module.h>
#include <drm/drm_atomic.h>
#include <drm/drm_atomic_constraints.h>
#include <drm/drm_atomic_helper.h>
#include <drm/drm_atomic_prepare.h>
#include <drm/drm_atomic_prepare_commit.h>
#include <drm/drm_atomic_prepare_outputs.h>
#include <drm/drm_atomic_prepare_ticket.h>
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
	unsigned int released;
	struct drm_prepare_source *source;
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

static void release_backend(void *data)
{
	struct test_backend *backend = data;

	WRITE_ONCE(backend->released, backend->released + 1);
}
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

static struct drm_atomic_commit *new_disable(struct kunit *test, struct atomic_fixture *f)
{
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
	ret = drm_atomic_set_mode_for_crtc(crtc, NULL);
	if (ret)
		goto out;
	crtc->active = false;
	plane = drm_atomic_get_plane_state(state, f->plane);
	if (IS_ERR(plane)) {
		ret = PTR_ERR(plane);
		goto out;
	}
	ret = drm_atomic_set_crtc_for_plane(plane, NULL);
	if (!ret)
		drm_atomic_set_fb_for_plane(plane, NULL);
out:
	unlock_update(state);
	return ret ? ERR_PTR(ret) : state;
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

static void closed_failed_backend_can_be_disabled(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_atomic_commit *first = new_update(test, f, f->target, f->tiled);
	struct drm_atomic_commit *stop;
	struct drm_constraints_entry *selected;
	unsigned int checks;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, first);
	KUNIT_ASSERT_EQ(test, run_update(first, drm_atomic_check_only), 0);
	KUNIT_ASSERT_EQ(test, run_update(first, swap_update), 0);
	f->backends[1].failed = true;
	checks = f->backends[1].checks;
	drm_constraints_catalog_close(drm_constraints_crtc_catalog(f->crtc));
	stop = new_disable(test, f);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, stop);
	KUNIT_ASSERT_EQ(test, run_update(stop, drm_atomic_check_only), 0);
	KUNIT_ASSERT_EQ(test, run_update(stop, swap_update), 0);
	KUNIT_EXPECT_FALSE(test, f->crtc->state->enable);
	KUNIT_EXPECT_FALSE(test, f->crtc->state->active);
	KUNIT_EXPECT_PTR_EQ(test, f->plane->state->fb, NULL);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, f->target);
	KUNIT_EXPECT_PTR_EQ(test, drm_atomic_get_old_crtc_state(stop, f->crtc)->constraints,
			    f->target);
	KUNIT_EXPECT_EQ(test, f->backends[1].checks, checks);
	selected = drm_constraints_catalog_selected(drm_constraints_crtc_catalog(f->crtc));
	KUNIT_EXPECT_PTR_EQ(test, selected, f->target);
	drm_constraints_entry_put(selected);
}

static void closure_after_check_still_permits_disable(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_atomic_commit *stop = new_disable(test, f);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, stop);
	KUNIT_ASSERT_EQ(test, run_update(stop, drm_atomic_check_only), 0);
	drm_constraints_catalog_close(drm_constraints_crtc_catalog(f->crtc));
	KUNIT_ASSERT_EQ(test, run_update(stop, swap_update), 0);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, f->initial);
}

static void closure_rejects_checked_activation(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_atomic_commit *state = new_update(test, f, f->target, f->tiled);
	struct drm_crtc_state *before = f->crtc->state;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	KUNIT_ASSERT_EQ(test, run_update(state, drm_atomic_check_only), 0);
	drm_constraints_catalog_close(drm_constraints_crtc_catalog(f->crtc));
	KUNIT_EXPECT_EQ(test, run_update(state, swap_update), -ESTALE);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state, before);
}

static void shutdown_cannot_select_through_closed_catalog(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_atomic_commit *stop = new_disable(test, f);
	struct drm_modeset_acquire_ctx ctx;
	int ret;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, stop);
	ret = lock_update(stop, &ctx);
	if (!ret)
		ret = drm_atomic_set_constraints_for_crtc(
			drm_atomic_get_new_crtc_state(stop, f->crtc), f->target);
	unlock_update(stop);
	KUNIT_ASSERT_EQ(test, ret, 0);
	KUNIT_ASSERT_EQ(test, run_update(stop, drm_atomic_check_only), 0);
	drm_constraints_catalog_close(drm_constraints_crtc_catalog(f->crtc));
	KUNIT_EXPECT_EQ(test, run_update(stop, swap_update), -ESTALE);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, f->initial);
}

static const char *read_fence_name(struct dma_fence *fence)
{
	return "constraints-native-read";
}

static const struct dma_fence_ops read_fence_ops = {
	.get_driver_name = read_fence_name,
	.get_timeline_name = read_fence_name,
};

static void finish_read(void *data)
{
	struct dma_fence *fence = data;

	dma_fence_signal(fence);
	dma_fence_put(fence);
}

static void put_source(void *data) { drm_prepare_source_put(data); }
static void put_ticket(void *data) { drm_prepare_ticket_put(data); }

static int observe_retiring_source(struct drm_atomic_commit *state,
				    struct drm_prepare_output_generation *entries,
				    unsigned int capacity)
{
	struct atomic_fixture *f = state->dev->dev_private;
	struct drm_crtc_state *old = drm_atomic_get_old_crtc_state(state, f->crtc);
	struct test_backend *backend = drm_constraints_entry_data(old->constraints);

	if (!capacity)
		return -ENOSPC;
	entries[0] = (struct drm_prepare_output_generation) {
		.crtc_id = f->crtc->base.id, .source = backend->source,
	};
	return 1;
}

struct retirement_worker {
	struct drm_atomic_commit *state;
	struct completion started;
	struct completion finished;
};

static int clear_retired_state(void *data)
{
	struct retirement_worker *worker = data;

	complete(&worker->started);
	drm_atomic_commit_clear(worker->state);
	complete(&worker->finished);
	set_current_state(TASK_INTERRUPTIBLE);
	while (!kthread_should_stop()) {
		schedule();
		set_current_state(TASK_INTERRUPTIBLE);
	}
	__set_current_state(TASK_RUNNING);
	return 0;
}

static void check_retained_native_read(struct kunit *test, bool failed_read, bool disable)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_constraints_catalog *catalog = drm_constraints_crtc_catalog(f->crtc);
	struct drm_constraints_entry *accepted = disable ? f->initial : f->target;
	struct drm_atomic_commit *first = new_update(test, f, NULL, f->linear);
	struct drm_prepare_output_generation output;
	struct drm_prepare_read_claim *claim;
	struct drm_prepare_ticket *ticket;
	struct drm_atomic_commit *next;
	struct dma_fence *fence;
	struct task_struct *task;
	struct retirement_worker worker;
	u64 old_id = drm_constraints_entry_id(f->initial);
	bool early;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, first);
	KUNIT_ASSERT_EQ(test, run_update(first, drm_atomic_check_only), 0);
	KUNIT_ASSERT_EQ(test, run_update(first, swap_update), 0);
	f->backends[0].source = drm_prepare_source_create(1);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->backends[0].source);
	KUNIT_ASSERT_EQ(test,
			kunit_add_action_or_reset(test, put_source, f->backends[0].source), 0);
	next = disable ? new_disable(test, f) : new_update(test, f, f->target, f->tiled);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, next);
	KUNIT_ASSERT_EQ(test, run_update(next, drm_atomic_check_only), 0);
	fence = kzalloc_obj(*fence);
	KUNIT_ASSERT_NOT_NULL(test, fence);
	dma_fence_init(fence, &read_fence_ops, NULL, dma_fence_context_alloc(1), 1);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, finish_read, fence), 0);
	claim = drm_prepare_source_claim(f->backends[0].source);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, claim);
	drm_prepare_read_release(claim, fence);
	output = (struct drm_prepare_output_generation) {
		.crtc_id = f->crtc->base.id, .source = f->backends[0].source,
	};
	ticket = drm_prepare_ticket_create(&output, 1);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, ticket);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_ticket, ticket), 0);
	KUNIT_ASSERT_EQ(test, drm_atomic_commit_prepare(next, ticket, observe_retiring_source), 0);
	if (disable) {
		drm_constraints_catalog_close(catalog);
		f->backends[0].failed = true;
	}
	KUNIT_ASSERT_EQ(test, run_update(next, swap_update), 0);
	KUNIT_EXPECT_FALSE(test, dma_fence_is_signaled(fence));
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, accepted);
	KUNIT_EXPECT_PTR_EQ(test, f->plane->state->fb, disable ? NULL : f->tiled);
	drm_prepare_ticket_cancel(ticket);
	if (!disable) {
		KUNIT_ASSERT_EQ(test, drm_constraints_catalog_withdraw(catalog, old_id), 0);
		KUNIT_ASSERT_EQ(test, drm_constraints_catalog_forget(catalog, old_id), 0);
	}
	drm_atomic_commit_clear(first);
	kunit_release_action(test, put_entry, f->initial);
	KUNIT_EXPECT_EQ(test, READ_ONCE(f->backends[0].released), 0);
	worker.state = next;
	init_completion(&worker.started);
	init_completion(&worker.finished);
	task = kthread_run(clear_retired_state, &worker, "constraints-retire");
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, task);
	wait_for_completion(&worker.started);
	early = wait_for_completion_timeout(&worker.finished, msecs_to_jiffies(20));
	KUNIT_EXPECT_EQ(test, READ_ONCE(f->backends[0].released), 0);
	claim = drm_prepare_source_claim(f->backends[0].source);
	KUNIT_EXPECT_TRUE(test, IS_ERR(claim) && PTR_ERR(claim) == -EBUSY);
	if (!IS_ERR(claim))
		drm_prepare_read_release(claim, NULL);
	if (failed_read)
		dma_fence_set_error(fence, -EIO);
	dma_fence_signal(fence);
	wait_for_completion(&worker.finished);
	kthread_stop(task);
	KUNIT_EXPECT_FALSE(test, early);
	KUNIT_EXPECT_EQ(test, READ_ONCE(f->backends[0].released), disable ? 0 : 1);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, accepted);
	claim = drm_prepare_source_claim(f->backends[0].source);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, claim);
	drm_prepare_read_release(claim, NULL);
}

static void predecessor_backend_survives_native_read(struct kunit *test)
{
	check_retained_native_read(test, false, false);
}

static void failed_read_retires_without_rolling_back_target(struct kunit *test)
{
	check_retained_native_read(test, true, false);
}

static void closed_output_shutdown_waits_for_native_read(struct kunit *test)
{
	check_retained_native_read(test, false, true);
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
	KUNIT_CASE(closed_failed_backend_can_be_disabled),
	KUNIT_CASE(closure_after_check_still_permits_disable),
	KUNIT_CASE(closure_rejects_checked_activation),
	KUNIT_CASE(shutdown_cannot_select_through_closed_catalog),
	KUNIT_CASE(predecessor_backend_survives_native_read),
	KUNIT_CASE(failed_read_retires_without_rolling_back_target),
	KUNIT_CASE(closed_output_shutdown_waits_for_native_read),
	{}
};

static struct kunit_suite drm_constraints_atomic_test_suite = {
	.name = "drm_constraints_atomic",
	.test_cases = drm_constraints_atomic_tests,
};

kunit_test_suite(drm_constraints_atomic_test_suite);

MODULE_LICENSE("Dual MIT/GPL");
