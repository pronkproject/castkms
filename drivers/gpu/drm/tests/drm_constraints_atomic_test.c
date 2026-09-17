// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/completion.h>
#include <linux/dma-fence.h>
#include <linux/file.h>
#include <linux/kthread.h>
#include <linux/module.h>
#include <drm/drm_atomic.h>
#include <drm/drm_atomic_constraints.h>
#include <drm/drm_atomic_helper.h>
#include <drm/drm_atomic_prepare.h>
#include <drm/drm_atomic_prepare_commit.h>
#include <drm/drm_atomic_prepare_display.h>
#include <drm/drm_atomic_prepare_outputs.h>
#include <drm/drm_atomic_prepare_ticket.h>
#include <drm/drm_atomic_request.h>
#include <drm/drm_atomic_uapi.h>
#include <drm/drm_auth.h>
#include <drm/drm_blend.h>
#include <drm/drm_color_mgmt.h>
#include <drm/drm_constraints.h>
#include <drm/drm_constraints_list.h>
#include <drm/drm_constraints_device.h>
#include <drm/drm_constraints_entry.h>
#include <drm/drm_constraints_output.h>
#include <drm/drm_constraints_owner.h>
#include <drm/drm_fourcc.h>
#include <drm/drm_file.h>
#include <drm/drm_framebuffer.h>
#include <drm/drm_ioctl.h>
#include <drm/drm_kunit_helpers.h>
#include <drm/drm_lease.h>
#include <drm/drm_modeset_helper.h>
#include <drm/drm_plane_helper.h>
#include <drm/drm_property.h>
#include <uapi/drm/drm_constraints.h>
#include <kunit/test.h>

#include "../drm_crtc_internal.h"
#include "../drm_internal.h"
#include "../drm_lease_internal.h"
#include "../drm_atomic_user_value.h"

static const struct file_operations test_fops = {
	.owner = THIS_MODULE,
	.release = drm_release_noglobal,
};

static const struct drm_driver test_driver = {
	.driver_features = DRIVER_MODESET | DRIVER_ATOMIC,
	.fops = &test_fops,
};

static void flush_owner_recovery(void *data)
{
	drm_constraints_owner_flush(data);
}

static void close_master_file(void *data)
{
	__fput_sync(data);
}

/* Metadata-only provider: framebuffer creation performs no GPU allocation. */
struct test_backend {
	bool failed;
	unsigned int checks;
	u64 checked_id;
	unsigned int released;
	struct drm_prepare_source *source;
};

struct atomic_fixture {
	struct drm_device *dev;
	struct drm_crtc *crtc;
	struct drm_plane *plane;
	struct drm_constraints_entry *initial;
	struct drm_constraints_entry *target;
	struct test_backend backends[3];
	struct drm_framebuffer *linear;
	struct drm_framebuffer *tiled;
	unsigned int installs;
	bool require_primary;
	struct completion *installed;
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

static int check_commit(struct drm_device *dev, struct drm_atomic_commit *state)
{
	struct atomic_fixture *f = dev->dev_private;
	struct drm_crtc_state *crtc;
	struct drm_crtc *output;
	int i;

	if (!f->require_primary)
		return 0;
	for_each_new_crtc_in_state(state, output, crtc, i)
		if (crtc->enable && !(crtc->plane_mask & drm_plane_mask(output->primary)))
			return -EINVAL;
	return 0;
}

static int commit_update(struct drm_device *dev, struct drm_atomic_commit *state, bool nonblock)
{
	struct atomic_fixture *f = dev->dev_private;
	int ret = drm_atomic_helper_swap_state(state, false);

	if (!ret) {
		f->installs++;
		if (f->installed)
			complete(f->installed);
	}
	return ret;
}

static const struct drm_mode_config_funcs mode_ops = {
	.fb_create = create_fb,
	.atomic_check = check_commit,
	.atomic_commit = commit_update,
};

static void release_backend(void *data)
{
	struct test_backend *backend = data;

	WRITE_ONCE(backend->released, backend->released + 1);
}
static const struct drm_constraints_entry_ops entry_ops = {
	.owner = THIS_MODULE,
	.release = release_backend,
};

static int check_backend(const struct drm_atomic_commit *state,
			 const struct drm_crtc_state *crtc,
			 const struct drm_constraints_entry *entry)
{
	struct test_backend *backend = drm_constraints_entry_data(entry);

	backend->checks++;
	backend->checked_id = drm_constraints_entry_id(entry);
	return backend->failed ? -EIO : 0;
}

static const struct drm_constraints_output_ops output_ops = { .check = check_backend };

static void put_description(void *data) { drm_constraints_description_put(data); }
static void put_entry(void *data) { drm_constraints_entry_put(data); }
static void put_fb(void *data) { drm_framebuffer_put(data); }

static struct drm_constraints_entry *
new_layout_entry(struct kunit *test, struct atomic_fixture *f, u32 format, u64 modifier,
		 unsigned int backend, const struct drm_constraints_property *rules,
		 unsigned int rule_count, bool implicit)
{
	const struct drm_constraints_size size = { 128, 64, 128, 64 };
	const struct drm_constraints_format allocation = {
		.plane_id = f->plane->base.id,
		.format = format,
		.modifier = modifier,
		.size = size,
		.flags = implicit ? DRM_CONSTRAINTS_FORMAT_IMPLICIT : 0,
		.storage_flags = DRM_CONSTRAINTS_FORMAT_STORAGE_NATIVE,
		.pitch_alignment = 1, .offset_alignment = 1, .max_pitch = U32_MAX,
	};
	struct drm_constraints_description *description;
	struct drm_constraints_entry *entry;

	description = drm_constraints_description_create(&size, &allocation, 1, rules, rule_count,
							 NULL, 0);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, description);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_description, description), 0);
	entry = drm_constraints_entry_create(drm_constraints_device_domain(f->dev), f->crtc->base.id,
					     description, &entry_ops, &f->backends[backend]);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, entry);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_entry, entry), 0);
	return entry;
}

static struct drm_constraints_entry *
new_entry(struct kunit *test, struct atomic_fixture *f, u32 format, u64 modifier,
	  unsigned int backend, const struct drm_constraints_property *rules,
	  unsigned int rule_count)
{
	return new_layout_entry(test, f, format, modifier, backend, rules, rule_count, false);
}

static struct drm_constraints_entry *
new_storage_entry(struct kunit *test, struct atomic_fixture *f, u32 storage_flags)
{
	const struct drm_constraints_size size = { 128, 64, 128, 64 };
	const struct drm_constraints_format allocation = {
		.plane_id = f->plane->base.id,
		.format = DRM_FORMAT_ARGB8888,
		.modifier = I915_FORMAT_MOD_X_TILED,
		.size = size,
		.storage_flags = storage_flags,
		.pitch_alignment = 256,
		.offset_alignment = 4096,
		.max_pitch = 1024,
	};
	struct drm_constraints_description *description;
	struct drm_constraints_entry *entry;

	description = drm_constraints_description_create(&size, &allocation, 1, NULL, 0, NULL, 0);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, description);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_description, description), 0);
	entry = drm_constraints_entry_create(drm_constraints_device_domain(f->dev),
		f->crtc->base.id, description, &entry_ops, &f->backends[2]);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, entry);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_entry, entry), 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_crtc_add(f->crtc, entry), 0);
	return entry;
}

static struct drm_framebuffer *
new_layout_fb(struct kunit *test, struct atomic_fixture *f, u32 format, u64 modifier,
	      u32 width, bool implicit)
{
	struct drm_mode_fb_cmd2 cmd = {
		.width = width, .height = 64, .pixel_format = format,
		.flags = implicit ? 0 : DRM_MODE_FB_MODIFIERS,
		.handles = { 1 }, .pitches = { width * 4 }, .modifier = { modifier },
	};
	struct drm_framebuffer *fb = drm_internal_framebuffer_create(f->dev, &cmd, NULL);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, fb);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_fb, fb), 0);
	return fb;
}

static struct drm_framebuffer *
new_fb(struct kunit *test, struct atomic_fixture *f, u32 format, u64 modifier, u32 width)
{
	return new_layout_fb(test, f, format, modifier, width, false);
}

static struct atomic_fixture *new_fixture_with_preparation(struct kunit *test, bool preparation)
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
	f->dev = __drm_kunit_helper_alloc_drm_device_with_driver(test, parent,
							      sizeof(*f->dev), 0, &test_driver);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->dev);
	f->dev->dev_private = f;
	f->dev->mode_config.funcs = &mode_ops;
	f->dev->mode_config.min_width = f->dev->mode_config.min_height = 1;
	f->dev->mode_config.max_width = f->dev->mode_config.max_height = 1024;
	if (preparation)
		KUNIT_ASSERT_EQ(test, drm_atomic_prepare_display_init(f->dev, 8), 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_device_init(f->dev, 8), 0);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, flush_owner_recovery, f->dev), 0);
	f->plane = drm_kunit_helper_create_primary_plane(test, f->dev, NULL, NULL,
							 formats, ARRAY_SIZE(formats), modifiers);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->plane);
	KUNIT_ASSERT_EQ(test, drm_plane_create_alpha_property(f->plane), 0);
	KUNIT_ASSERT_EQ(test, drm_plane_create_zpos_property(f->plane, 0, 0, 3), 0);
	KUNIT_ASSERT_EQ(test, drm_plane_create_rotation_property(f->plane,
			DRM_MODE_ROTATE_0, DRM_MODE_ROTATE_0 | DRM_MODE_ROTATE_90), 0);
	KUNIT_ASSERT_EQ(test, drm_plane_create_color_properties(f->plane,
			BIT(DRM_COLOR_YCBCR_BT601) | BIT(DRM_COLOR_YCBCR_BT709),
			BIT(DRM_COLOR_YCBCR_LIMITED_RANGE) | BIT(DRM_COLOR_YCBCR_FULL_RANGE),
			DRM_COLOR_YCBCR_BT601, DRM_COLOR_YCBCR_LIMITED_RANGE), 0);
	f->crtc = drm_kunit_helper_create_crtc(test, f->dev, f->plane, NULL, NULL, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->crtc);
	drm_mode_config_reset(f->dev);
	f->initial = new_entry(test, f, DRM_FORMAT_XRGB8888, DRM_FORMAT_MOD_LINEAR, 0, NULL, 0);
	f->target = new_entry(test, f, DRM_FORMAT_ARGB8888, I915_FORMAT_MOD_X_TILED, 1, NULL, 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_crtc_init(f->crtc, f->initial, 4, &output_ops), 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_crtc_add(f->crtc, f->target), 0);
	f->linear = new_fb(test, f, DRM_FORMAT_XRGB8888, DRM_FORMAT_MOD_LINEAR, 128);
	f->tiled = new_fb(test, f, DRM_FORMAT_ARGB8888, I915_FORMAT_MOD_X_TILED, 128);
	return f;
}

static struct atomic_fixture *new_fixture(struct kunit *test)
{
	return new_fixture_with_preparation(test, false);
}

/* Allocate f before the shared device so backend data outlives its cleanup. */
static void init_additional_output(struct kunit *test, struct atomic_fixture *f,
				   struct drm_device *dev)
{
	static const u32 formats[] = { DRM_FORMAT_XRGB8888, DRM_FORMAT_ARGB8888 };
	static const u64 modifiers[] = {
		DRM_FORMAT_MOD_LINEAR, I915_FORMAT_MOD_X_TILED, DRM_FORMAT_MOD_INVALID,
	};

	f->dev = dev;
	f->plane = drm_kunit_helper_create_primary_plane(test, dev, NULL, NULL,
							 formats, ARRAY_SIZE(formats), modifiers);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->plane);
	f->crtc = drm_kunit_helper_create_crtc(test, dev, f->plane, NULL, NULL, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->crtc);
	/* All outputs must exist before allocating any atomic transactions. */
	drm_mode_config_reset(dev);
	f->initial = new_entry(test, f, DRM_FORMAT_XRGB8888, DRM_FORMAT_MOD_LINEAR, 0, NULL, 0);
	f->target = new_entry(test, f, DRM_FORMAT_ARGB8888, I915_FORMAT_MOD_X_TILED, 1, NULL, 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_crtc_init(f->crtc, f->initial, 4, &output_ops), 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_crtc_add(f->crtc, f->target), 0);
	f->linear = new_fb(test, f, DRM_FORMAT_XRGB8888, DRM_FORMAT_MOD_LINEAR, 128);
	f->tiled = new_fb(test, f, DRM_FORMAT_ARGB8888, I915_FORMAT_MOD_X_TILED, 128);
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
	selected = drm_constraints_list_selected(drm_constraints_crtc_list(f->crtc));
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_entry, selected), 0);
	KUNIT_EXPECT_PTR_EQ(test, selected, f->initial);
	KUNIT_EXPECT_EQ(test, f->backends[0].checks, 0);
	KUNIT_EXPECT_EQ(test, f->backends[1].checks, 1);
	KUNIT_EXPECT_EQ(test, f->backends[1].checked_id, drm_constraints_entry_id(f->target));
}

static void atomic_layout_matching_distinguishes_implicit_from_linear(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_constraints_entry *implicit =
		new_layout_entry(test, f, DRM_FORMAT_XRGB8888, 0, 2, NULL, 0, true);
	struct drm_framebuffer *fb = new_layout_fb(test, f, DRM_FORMAT_XRGB8888, 0, 128, true);
	struct drm_atomic_commit *state;

	KUNIT_ASSERT_EQ(test, drm_constraints_crtc_add(f->crtc, implicit), 0);
	state = new_update(test, f, f->initial, fb);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	KUNIT_EXPECT_EQ(test, run_update(state, check_update), -EINVAL);
	drm_atomic_commit_clear(state);
	state = new_update(test, f, implicit, f->linear);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	KUNIT_EXPECT_EQ(test, run_update(state, check_update), -EINVAL);
	drm_atomic_commit_clear(state);
	state = new_update(test, f, implicit, fb);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	KUNIT_EXPECT_EQ(test, run_update(state, check_update), 0);
	drm_atomic_commit_clear(state);
	/* Model a driver's resolved implicit storage before submitting it again. */
	fb->modifier = I915_FORMAT_MOD_X_TILED;
	state = new_update(test, f, implicit, fb);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	KUNIT_EXPECT_EQ(test, run_update(state, check_update), 0);
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
	KUNIT_ASSERT_EQ(test, drm_constraints_list_withdraw(drm_constraints_crtc_list(f->crtc),
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

static void source_allocation_respects_storage_requirements(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_constraints_entry *native = new_storage_entry(
		test, f, DRM_CONSTRAINTS_FORMAT_STORAGE_NATIVE);
	struct drm_constraints_entry *imported = new_storage_entry(
		test, f, DRM_CONSTRAINTS_FORMAT_STORAGE_IMPORTED);
	struct drm_atomic_commit *state = new_update(test, f, native, f->tiled);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	KUNIT_EXPECT_EQ(test, run_update(state, drm_atomic_check_only), 0);
	f->tiled->pitches[0]++;
	KUNIT_EXPECT_EQ(test, run_update(state, drm_atomic_check_only), -EINVAL);
	f->tiled->pitches[0] = 1280;
	KUNIT_EXPECT_EQ(test, run_update(state, drm_atomic_check_only), -EINVAL);
	f->tiled->pitches[0] = 512;
	f->tiled->offsets[0] = 1;
	KUNIT_EXPECT_EQ(test, run_update(state, drm_atomic_check_only), -EINVAL);
	f->tiled->offsets[0] = 0;
	KUNIT_EXPECT_EQ(test, run_update(state, drm_atomic_check_only), 0);
	drm_atomic_commit_clear(state);
	state = new_update(test, f, imported, f->tiled);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	KUNIT_EXPECT_EQ(test, run_update(state, drm_atomic_check_only), -EINVAL);
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

static void transactions_may_include_outputs_without_constraints(struct kunit *test)
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
	KUNIT_EXPECT_EQ(test, run_update(state, check_update), 0);
	KUNIT_EXPECT_EQ(test, run_update(state, install_update), 0);
	KUNIT_EXPECT_EQ(test, f->installs, 1);
}

static int swap_update(struct drm_atomic_commit *state)
{
	return drm_atomic_helper_swap_state(state, false);
}

static int rebind_live_state(struct drm_atomic_commit *state)
{
	struct atomic_fixture *f = state->dev->dev_private;

	return drm_atomic_set_constraints_for_crtc(f->crtc->state, f->target);
}

static int rebind_unowned_state(struct drm_atomic_commit *state)
{
	struct atomic_fixture *f = state->dev->dev_private;
	struct drm_crtc_state *detached = f->crtc->funcs->atomic_duplicate_state(f->crtc);
	int ret;

	if (!detached)
		return -ENOMEM;
	detached->state = state;
	ret = drm_atomic_set_constraints_for_crtc(detached, f->target);
	f->crtc->funcs->atomic_destroy_state(f->crtc, detached);
	return ret;
}

static int rebind_proposed_state(struct drm_atomic_commit *state)
{
	struct atomic_fixture *f = state->dev->dev_private;

	return drm_atomic_set_constraints_for_crtc(
		drm_atomic_get_new_crtc_state(state, f->crtc), f->target);
}

static int rebind_retiring_state(struct drm_atomic_commit *state)
{
	struct atomic_fixture *f = state->dev->dev_private;

	return drm_atomic_set_constraints_for_crtc(
		drm_atomic_get_old_crtc_state(state, f->crtc), f->target);
}

static void selection_requires_owned_mutable_proposed_state(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_atomic_commit *state = new_update(test, f, NULL, f->linear);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	KUNIT_EXPECT_EQ(test, run_update(state, rebind_live_state), -EINVAL);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, f->initial);
	KUNIT_EXPECT_EQ(test, run_update(state, rebind_unowned_state), -EINVAL);
	KUNIT_EXPECT_PTR_EQ(test, drm_atomic_get_new_crtc_state(state, f->crtc)->constraints,
			    f->initial);
	KUNIT_ASSERT_EQ(test, run_update(state, drm_atomic_check_only), 0);
	KUNIT_EXPECT_EQ(test, run_update(state, rebind_proposed_state), -EBUSY);
	KUNIT_EXPECT_PTR_EQ(test, drm_atomic_get_new_crtc_state(state, f->crtc)->constraints,
			    f->initial);
	KUNIT_ASSERT_EQ(test, run_update(state, swap_update), 0);
	KUNIT_EXPECT_EQ(test, run_update(state, rebind_live_state), -EINVAL);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, f->initial);
	KUNIT_EXPECT_EQ(test, run_update(state, rebind_retiring_state), -EINVAL);
	KUNIT_EXPECT_PTR_EQ(test, drm_atomic_get_old_crtc_state(state, f->crtc)->constraints,
			    f->initial);
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
	selected = drm_constraints_list_selected(drm_constraints_crtc_list(f->crtc));
	KUNIT_EXPECT_PTR_EQ(test, selected, f->target);
	drm_constraints_entry_put(selected);
}

static void state_swap_rechecks_withdrawn_target(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_constraints_list *list = drm_constraints_crtc_list(f->crtc);
	struct drm_atomic_commit *state = new_update(test, f, f->target, f->tiled);
	struct drm_crtc_state *before = f->crtc->state;
	struct drm_plane_state *plane_before = f->plane->state;
	struct drm_constraints_entry *selected;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	KUNIT_ASSERT_EQ(test, run_update(state, drm_atomic_check_only), 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_list_withdraw(list,
						      drm_constraints_entry_id(f->target)), 0);
	KUNIT_EXPECT_EQ(test, run_update(state, swap_update), -ESTALE);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state, before);
	KUNIT_EXPECT_PTR_EQ(test, f->plane->state, plane_before);
	selected = drm_constraints_list_selected(drm_constraints_crtc_list(f->crtc));
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
	struct drm_constraints_list *list = drm_constraints_crtc_list(f->crtc);
	struct drm_atomic_commit *first = new_update(test, f, f->target, f->tiled);
	struct drm_atomic_commit *next;
	struct drm_constraints_snapshot *snapshot;
	u64 generation;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, first);
	KUNIT_ASSERT_EQ(test, run_update(first, drm_atomic_check_only), 0);
	KUNIT_ASSERT_EQ(test, run_update(first, swap_update), 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_list_withdraw(list,
						      drm_constraints_entry_id(f->target)), 0);
	snapshot = drm_constraints_list_snapshot(list, 0);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, snapshot);
	generation = drm_constraints_snapshot_info(snapshot)->generation;
	drm_constraints_snapshot_put(snapshot);
	next = new_update(test, f, NULL, f->tiled);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, next);
	next->allow_modeset = false;
	KUNIT_ASSERT_EQ(test, run_update(next, drm_atomic_check_only), 0);
	KUNIT_ASSERT_EQ(test, run_update(next, swap_update), 0);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, f->target);
	snapshot = drm_constraints_list_snapshot(list, generation);
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
	drm_constraints_list_close(drm_constraints_crtc_list(f->crtc));
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
	selected = drm_constraints_list_selected(drm_constraints_crtc_list(f->crtc));
	KUNIT_EXPECT_PTR_EQ(test, selected, f->target);
	drm_constraints_entry_put(selected);
}

static void closure_after_check_still_permits_disable(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_atomic_commit *stop = new_disable(test, f);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, stop);
	KUNIT_ASSERT_EQ(test, run_update(stop, drm_atomic_check_only), 0);
	drm_constraints_list_close(drm_constraints_crtc_list(f->crtc));
	KUNIT_ASSERT_EQ(test, run_update(stop, swap_update), 0);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, f->initial);
}

static void check_framebuffer_removal(struct kunit *test, bool unavailable)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_atomic_commit *first = new_update(test, f, f->target, f->tiled);
	struct drm_constraints_entry *selected;
	unsigned int checks;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, first);
	KUNIT_ASSERT_EQ(test, run_update(first, drm_atomic_commit), 0);
	drm_atomic_commit_clear(first);
	if (unavailable) {
		/* Model a driver which needs its primary plane whenever enabled. */
		f->require_primary = true;
		f->backends[1].failed = true;
		drm_constraints_list_close(drm_constraints_crtc_list(f->crtc));
	}
	checks = f->backends[1].checks;
	/* Removal consumes its own reference, separate from fixture ownership. */
	drm_framebuffer_get(f->tiled);
	drm_framebuffer_remove(f->tiled);
	KUNIT_EXPECT_EQ(test, f->installs, 2);
	KUNIT_EXPECT_PTR_EQ(test, f->plane->state->fb, NULL);
	KUNIT_EXPECT_PTR_EQ(test, f->plane->state->crtc, NULL);
	KUNIT_EXPECT_EQ(test, f->crtc->state->enable, !unavailable);
	KUNIT_EXPECT_EQ(test, f->crtc->state->active, !unavailable);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, f->target);
	selected = drm_constraints_list_selected(drm_constraints_crtc_list(f->crtc));
	KUNIT_EXPECT_PTR_EQ(test, selected, f->target);
	drm_constraints_entry_put(selected);
	if (unavailable)
		KUNIT_EXPECT_EQ(test, f->backends[1].checks, checks);
}

static void framebuffer_removal_preserves_accepted_binding(struct kunit *test)
{
	check_framebuffer_removal(test, false);
}

static void framebuffer_removal_can_disable_unavailable_output(struct kunit *test)
{
	check_framebuffer_removal(test, true);
}

static void default_restoration_requires_quiescent_output(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_atomic_commit *state = new_update(test, f, f->target, f->tiled);
	struct drm_constraints_entry *selected;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	KUNIT_ASSERT_EQ(test, run_update(state, drm_atomic_commit), 0);
	drm_atomic_commit_clear(state);
	KUNIT_EXPECT_EQ(test, drm_atomic_constraints_restore_default(f->crtc), -EBUSY);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, f->target);
	KUNIT_EXPECT_PTR_EQ(test, f->plane->state->fb, f->tiled);
	KUNIT_EXPECT_EQ(test, f->installs, 1);
	f->backends[1].failed = true;
	drm_atomic_helper_shutdown(f->dev);
	KUNIT_ASSERT_FALSE(test, f->crtc->state->enable);
	KUNIT_ASSERT_EQ(test, drm_atomic_constraints_restore_default(f->crtc), 0);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, f->initial);
	KUNIT_EXPECT_PTR_EQ(test, f->plane->state->fb, NULL);
	KUNIT_EXPECT_FALSE(test, f->crtc->state->active);
	KUNIT_EXPECT_EQ(test, f->installs, 3);
	selected = drm_constraints_list_selected(drm_constraints_crtc_list(f->crtc));
	KUNIT_EXPECT_PTR_EQ(test, selected, f->initial);
	drm_constraints_entry_put(selected);
	KUNIT_ASSERT_EQ(test, drm_atomic_constraints_restore_default(f->crtc), 0);
	KUNIT_EXPECT_EQ(test, f->installs, 3);
}

static void default_restoration_rechecks_default_availability(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_atomic_commit *state = new_update(test, f, f->target, f->tiled);
	struct drm_constraints_list *list = drm_constraints_crtc_list(f->crtc);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	KUNIT_ASSERT_EQ(test, run_update(state, drm_atomic_commit), 0);
	drm_atomic_commit_clear(state);
	drm_atomic_helper_shutdown(f->dev);
	KUNIT_ASSERT_FALSE(test, f->crtc->state->enable);
	f->backends[0].failed = true;
	KUNIT_EXPECT_EQ(test, drm_atomic_constraints_restore_default(f->crtc), -EIO);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, f->target);
	KUNIT_EXPECT_EQ(test, f->installs, 2);
	f->backends[0].failed = false;
	KUNIT_ASSERT_EQ(test,
		drm_constraints_list_withdraw(list, drm_constraints_entry_id(f->initial)), 0);
	KUNIT_EXPECT_EQ(test, drm_atomic_constraints_restore_default(f->crtc), -ESTALE);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, f->target);
	drm_constraints_list_close(list);
	KUNIT_EXPECT_EQ(test, drm_atomic_constraints_restore_default(f->crtc), -ESTALE);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, f->target);
	KUNIT_EXPECT_EQ(test, f->installs, 2);
}

static void closed_default_is_not_successful_restoration(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);

	drm_constraints_list_close(drm_constraints_crtc_list(f->crtc));
	KUNIT_EXPECT_EQ(test, drm_atomic_constraints_restore_default(f->crtc), -ESTALE);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, f->initial);
	KUNIT_EXPECT_EQ(test, f->installs, 0);
}

static void recovery_restores_all_defaults_before_retiring_offers(struct kunit *test)
{
	struct atomic_fixture *other = kunit_kzalloc(test, sizeof(*other), GFP_KERNEL);
	struct atomic_fixture *f = new_fixture(test);
	struct atomic_fixture *outputs[] = { f, other };
	struct drm_constraints_snapshot *snapshot;
	unsigned int i;

	KUNIT_ASSERT_NOT_NULL(test, other);
	init_additional_output(test, other, f->dev);
	for (i = 0; i < ARRAY_SIZE(outputs); i++) {
		struct atomic_fixture *output = outputs[i];
		struct drm_atomic_commit *state = new_update(test, output,
							    output->target, output->tiled);

		KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
		KUNIT_ASSERT_EQ(test, run_update(state, drm_atomic_commit), 0);
		drm_atomic_commit_clear(state);
		output->backends[1].failed = true;
	}
	other->backends[0].failed = true;
	KUNIT_EXPECT_EQ(test, drm_constraints_recover(f->dev), -EIO);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, f->initial);
	KUNIT_EXPECT_PTR_EQ(test, other->crtc->state->constraints, other->target);
	for (i = 0; i < ARRAY_SIZE(outputs); i++) {
		KUNIT_EXPECT_FALSE(test, outputs[i]->crtc->state->enable);
		KUNIT_EXPECT_PTR_EQ(test, outputs[i]->plane->state->fb, NULL);
		snapshot = drm_constraints_list_snapshot(
			drm_constraints_crtc_list(outputs[i]->crtc), 0);
		KUNIT_ASSERT_NOT_ERR_OR_NULL(test, snapshot);
		KUNIT_EXPECT_EQ(test, drm_constraints_snapshot_info(snapshot)->count, 2);
		drm_constraints_snapshot_put(snapshot);
	}
	other->backends[0].failed = false;
	KUNIT_ASSERT_EQ(test, drm_constraints_recover(f->dev), 0);
	for (i = 0; i < ARRAY_SIZE(outputs); i++) {
		struct atomic_fixture *output = outputs[i];

		KUNIT_EXPECT_PTR_EQ(test, output->crtc->state->constraints, output->initial);
		snapshot = drm_constraints_list_snapshot(
			drm_constraints_crtc_list(output->crtc), 0);
		KUNIT_ASSERT_NOT_ERR_OR_NULL(test, snapshot);
		KUNIT_EXPECT_EQ(test, drm_constraints_snapshot_info(snapshot)->count, 1);
		KUNIT_EXPECT_EQ(test, drm_constraints_snapshot_info(snapshot)->selected_id,
				 drm_constraints_entry_id(output->initial));
		drm_constraints_snapshot_put(snapshot);
	}
}

static void recovery_does_not_reopen_closed_lists(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_atomic_commit *state = new_update(test, f, f->target, f->tiled);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	KUNIT_ASSERT_EQ(test, run_update(state, drm_atomic_commit), 0);
	drm_atomic_commit_clear(state);
	drm_constraints_list_close(drm_constraints_crtc_list(f->crtc));
	KUNIT_EXPECT_EQ(test, drm_constraints_recover(f->dev), -ESTALE);
	KUNIT_EXPECT_FALSE(test, f->crtc->state->enable);
	KUNIT_EXPECT_PTR_EQ(test, f->plane->state->fb, NULL);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, f->target);
}

static int owner_status(struct drm_device *dev)
{
	int ret;

	mutex_lock(&dev->master_mutex);
	ret = drm_constraints_owner_check(dev);
	mutex_unlock(&dev->master_mutex);
	return ret;
}

static void owner_recovery_excludes_replacement_until_success(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_atomic_commit *state = new_update(test, f, f->target, f->tiled);
	int pending;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	KUNIT_ASSERT_EQ(test, run_update(state, drm_atomic_commit), 0);
	drm_atomic_commit_clear(state);
	KUNIT_EXPECT_EQ(test, owner_status(f->dev), 0);
	f->backends[0].failed = true;
	mutex_lock(&f->dev->master_mutex);
	drm_constraints_owner_lost(f->dev);
	drm_constraints_owner_lost(f->dev);
	pending = drm_constraints_owner_check(f->dev);
	mutex_unlock(&f->dev->master_mutex);
	drm_constraints_owner_flush(f->dev);
	KUNIT_EXPECT_EQ(test, pending, -EBUSY);
	KUNIT_EXPECT_EQ(test, owner_status(f->dev), -EIO);
	KUNIT_EXPECT_FALSE(test, f->crtc->state->enable);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, f->target);
	f->backends[0].failed = false;
	mutex_lock(&f->dev->master_mutex);
	drm_constraints_owner_retry(f->dev);
	pending = drm_constraints_owner_check(f->dev);
	mutex_unlock(&f->dev->master_mutex);
	drm_constraints_owner_flush(f->dev);
	KUNIT_EXPECT_EQ(test, pending, -EBUSY);
	KUNIT_EXPECT_EQ(test, owner_status(f->dev), 0);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, f->initial);
}

static void owner_recovery_stops_after_unplug_or_cleanup(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);

	f->dev->unplugged = true;
	mutex_lock(&f->dev->master_mutex);
	drm_constraints_owner_lost(f->dev);
	mutex_unlock(&f->dev->master_mutex);
	drm_constraints_owner_flush(f->dev);
	KUNIT_EXPECT_EQ(test, owner_status(f->dev), -ENODEV);
	KUNIT_EXPECT_EQ(test, f->installs, 0);
	f->dev->unplugged = false;
	drm_constraints_owner_stop(f->dev);
	mutex_lock(&f->dev->master_mutex);
	drm_constraints_owner_retry(f->dev);
	drm_constraints_owner_lost(f->dev);
	mutex_unlock(&f->dev->master_mutex);
	drm_constraints_owner_flush(f->dev);
	KUNIT_EXPECT_EQ(test, owner_status(f->dev), -ENODEV);
	KUNIT_EXPECT_EQ(test, f->installs, 0);
}

static void closure_rejects_checked_activation(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_atomic_commit *state = new_update(test, f, f->target, f->tiled);
	struct drm_crtc_state *before = f->crtc->state;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	KUNIT_ASSERT_EQ(test, run_update(state, drm_atomic_check_only), 0);
	drm_constraints_list_close(drm_constraints_crtc_list(f->crtc));
	KUNIT_EXPECT_EQ(test, run_update(state, swap_update), -ESTALE);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state, before);
}

static void shutdown_cannot_select_through_closed_list(struct kunit *test)
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
	drm_constraints_list_close(drm_constraints_crtc_list(f->crtc));
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

static void check_retained_native_read(struct kunit *test, bool failed_read, bool disable,
				      bool independent_output)
{
	struct atomic_fixture *other = kunit_kzalloc(test, sizeof(*other), GFP_KERNEL);
	struct atomic_fixture *f = new_fixture(test);
	struct drm_constraints_list *list = drm_constraints_crtc_list(f->crtc);
	struct drm_constraints_entry *retiring = new_entry(test, f, DRM_FORMAT_XRGB8888,
						 DRM_FORMAT_MOD_LINEAR, 2, NULL, 0);
	struct drm_constraints_entry *accepted = disable ? retiring : f->target;
	struct drm_atomic_commit *first;
	struct drm_atomic_commit *other_update = NULL;
	struct drm_prepare_output_generation output;
	struct drm_prepare_read_claim *claim;
	struct drm_prepare_ticket *ticket;
	struct drm_atomic_commit *next;
	struct dma_fence *fence;
	struct task_struct *task;
	struct retirement_worker worker;
	u64 old_id = drm_constraints_entry_id(retiring);
	bool early;

	KUNIT_ASSERT_NOT_NULL(test, other);
	if (independent_output) {
		init_additional_output(test, other, f->dev);
		other_update = new_update(test, other, other->target, other->tiled);
		KUNIT_ASSERT_NOT_ERR_OR_NULL(test, other_update);
		KUNIT_ASSERT_EQ(test, run_update(other_update, drm_atomic_check_only), 0);
	}
	KUNIT_ASSERT_EQ(test, drm_constraints_crtc_add(f->crtc, retiring), 0);
	first = new_update(test, f, retiring, f->linear);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, first);
	KUNIT_ASSERT_EQ(test, run_update(first, drm_atomic_check_only), 0);
	KUNIT_ASSERT_EQ(test, run_update(first, swap_update), 0);
	f->backends[2].source = drm_prepare_source_create(1);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->backends[2].source);
	KUNIT_ASSERT_EQ(test,
			kunit_add_action_or_reset(test, put_source, f->backends[2].source), 0);
	next = disable ? new_disable(test, f) : new_update(test, f, f->target, f->tiled);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, next);
	KUNIT_ASSERT_EQ(test, run_update(next, drm_atomic_check_only), 0);
	fence = kzalloc_obj(*fence);
	KUNIT_ASSERT_NOT_NULL(test, fence);
	dma_fence_init(fence, &read_fence_ops, NULL, dma_fence_context_alloc(1), 1);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, finish_read, fence), 0);
	claim = drm_prepare_source_claim(f->backends[2].source);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, claim);
	drm_prepare_read_release(claim, fence);
	output = (struct drm_prepare_output_generation) {
		.crtc_id = f->crtc->base.id, .source = f->backends[2].source,
	};
	ticket = drm_prepare_ticket_create(&output, 1);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, ticket);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_ticket, ticket), 0);
	KUNIT_ASSERT_EQ(test, drm_atomic_commit_prepare(next, ticket, observe_retiring_source), 0);
	if (disable) {
		drm_constraints_list_close(list);
		f->backends[2].failed = true;
	}
	KUNIT_ASSERT_EQ(test, run_update(next, swap_update), 0);
	KUNIT_EXPECT_FALSE(test, dma_fence_is_signaled(fence));
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, accepted);
	KUNIT_EXPECT_PTR_EQ(test, f->plane->state->fb, disable ? NULL : f->tiled);
	drm_prepare_ticket_cancel(ticket);
	if (!disable) {
		KUNIT_ASSERT_EQ(test, drm_constraints_list_withdraw(list, old_id), 0);
		KUNIT_ASSERT_EQ(test, drm_constraints_list_forget(list, old_id), 0);
	}
	drm_atomic_commit_clear(first);
	kunit_release_action(test, put_entry, retiring);
	KUNIT_EXPECT_EQ(test, READ_ONCE(f->backends[2].released), 0);
	worker.state = next;
	init_completion(&worker.started);
	init_completion(&worker.finished);
	task = kthread_run(clear_retired_state, &worker, "constraints-retire");
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, task);
	wait_for_completion(&worker.started);
	early = wait_for_completion_timeout(&worker.finished, msecs_to_jiffies(20));
	KUNIT_EXPECT_EQ(test, READ_ONCE(f->backends[2].released), 0);
	claim = drm_prepare_source_claim(f->backends[2].source);
	KUNIT_EXPECT_TRUE(test, IS_ERR(claim) && PTR_ERR(claim) == -EBUSY);
	if (!IS_ERR(claim))
		drm_prepare_read_release(claim, NULL);
	if (other_update) {
		KUNIT_EXPECT_EQ(test, run_update(other_update, swap_update), 0);
		drm_atomic_commit_clear(other_update);
		KUNIT_EXPECT_PTR_EQ(test, other->crtc->state->constraints, other->target);
		KUNIT_EXPECT_PTR_EQ(test, other->plane->state->fb, other->tiled);
		KUNIT_EXPECT_FALSE(test, completion_done(&worker.finished));
		KUNIT_EXPECT_FALSE(test, dma_fence_is_signaled(fence));
	}
	if (failed_read)
		dma_fence_set_error(fence, -EIO);
	dma_fence_signal(fence);
	wait_for_completion(&worker.finished);
	kthread_stop(task);
	KUNIT_EXPECT_FALSE(test, early);
	KUNIT_EXPECT_EQ(test, READ_ONCE(f->backends[2].released), disable ? 0 : 1);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, accepted);
	claim = drm_prepare_source_claim(f->backends[2].source);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, claim);
	drm_prepare_read_release(claim, NULL);
}

static void predecessor_backend_survives_native_read(struct kunit *test)
{
	check_retained_native_read(test, false, false, false);
}

static void failed_read_retires_without_rolling_back_target(struct kunit *test)
{
	check_retained_native_read(test, true, false, false);
}

static void closed_output_shutdown_waits_for_native_read(struct kunit *test)
{
	check_retained_native_read(test, false, true, false);
}

static void native_read_retirement_does_not_stall_another_output(struct kunit *test)
{
	check_retained_native_read(test, false, false, true);
}

static void independent_outputs_keep_exact_bindings_during_animation(struct kunit *test)
{
	struct atomic_fixture *other = kunit_kzalloc(test, sizeof(*other), GFP_KERNEL);
	struct atomic_fixture *f = new_fixture(test);
	struct atomic_fixture *outputs[] = { f, other };
	unsigned int frame, output;

	KUNIT_ASSERT_NOT_NULL(test, other);
	init_additional_output(test, other, f->dev);
	KUNIT_EXPECT_NE(test, drm_constraints_entry_id(f->initial),
			drm_constraints_entry_id(other->initial));
	KUNIT_EXPECT_NE(test, drm_constraints_entry_id(f->target),
			drm_constraints_entry_id(other->target));
	for (frame = 0; frame < 32; frame++) {
		for (output = 0; output < ARRAY_SIZE(outputs); output++) {
			struct atomic_fixture *active_output = outputs[output];
			struct atomic_fixture *idle = outputs[1 - output];
			struct drm_crtc_state *idle_state = idle->crtc->state;
			struct drm_constraints_list *list =
				drm_constraints_crtc_list(idle->crtc);
			struct drm_constraints_snapshot *snapshot;
			struct drm_constraints_entry *entry, *selected;
			struct drm_atomic_commit *state;
			u64 generation;
			bool tiled = (frame + output) % 2;

			snapshot = drm_constraints_list_snapshot(list, 0);
			KUNIT_ASSERT_NOT_ERR_OR_NULL(test, snapshot);
			generation = drm_constraints_snapshot_info(snapshot)->generation;
			drm_constraints_snapshot_put(snapshot);
			entry = tiled ? active_output->target : active_output->initial;
			state = new_update(test, active_output, entry,
					   tiled ? active_output->tiled : active_output->linear);
			KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
			KUNIT_ASSERT_EQ(test, run_update(state, drm_atomic_check_only), 0);
			KUNIT_ASSERT_EQ(test, run_update(state, swap_update), 0);
			drm_atomic_commit_clear(state);
			KUNIT_EXPECT_PTR_EQ(test, active_output->crtc->state->constraints, entry);
			KUNIT_EXPECT_PTR_EQ(test, active_output->plane->state->fb,
					   tiled ? active_output->tiled : active_output->linear);
			selected = drm_constraints_list_selected(
					drm_constraints_crtc_list(active_output->crtc));
			KUNIT_EXPECT_PTR_EQ(test, selected, entry);
			drm_constraints_entry_put(selected);
			KUNIT_EXPECT_PTR_EQ(test, idle->crtc->state, idle_state);
			snapshot = drm_constraints_list_snapshot(list, generation);
			KUNIT_ASSERT_NOT_ERR_OR_NULL(test, snapshot);
			drm_constraints_snapshot_put(snapshot);
		}
	}
}

static void shutdown_disables_all_unavailable_outputs(struct kunit *test)
{
	struct atomic_fixture *other = kunit_kzalloc(test, sizeof(*other), GFP_KERNEL);
	struct atomic_fixture *f = new_fixture(test);
	struct atomic_fixture *outputs[] = { f, other };
	struct drm_constraints_entry *bindings[2];
	unsigned int checks[2], i;

	KUNIT_ASSERT_NOT_NULL(test, other);
	init_additional_output(test, other, f->dev);
	bindings[0] = f->target;
	bindings[1] = other->initial;
	for (i = 0; i < ARRAY_SIZE(outputs); i++) {
		struct atomic_fixture *output = outputs[i];
		struct drm_atomic_commit *state = new_update(test, output, bindings[i],
							    i ? output->linear : output->tiled);

		KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
		KUNIT_ASSERT_EQ(test, run_update(state, drm_atomic_commit), 0);
		drm_atomic_commit_clear(state);
		output->backends[1 - i].failed = true;
		checks[i] = output->backends[1 - i].checks;
		drm_constraints_list_close(drm_constraints_crtc_list(output->crtc));
	}
	drm_atomic_helper_shutdown(f->dev);
	KUNIT_EXPECT_EQ(test, f->installs, 3);
	for (i = 0; i < ARRAY_SIZE(outputs); i++) {
		struct atomic_fixture *output = outputs[i];
		struct drm_constraints_entry *selected;

		KUNIT_EXPECT_FALSE(test, output->crtc->state->enable);
		KUNIT_EXPECT_FALSE(test, output->crtc->state->active);
		KUNIT_EXPECT_EQ(test, output->crtc->state->plane_mask, 0);
		KUNIT_EXPECT_PTR_EQ(test, output->plane->state->fb, NULL);
		KUNIT_EXPECT_PTR_EQ(test, output->crtc->state->constraints, bindings[i]);
		KUNIT_EXPECT_EQ(test, output->backends[1 - i].checks, checks[i]);
		selected = drm_constraints_list_selected(drm_constraints_crtc_list(output->crtc));
		KUNIT_EXPECT_PTR_EQ(test, selected, bindings[i]);
		drm_constraints_entry_put(selected);
	}
}

static void constraints_cohort_acceptance(struct kunit *test, bool withdraw, bool fail)
{
	struct atomic_fixture *other = kunit_kzalloc(test, sizeof(*other), GFP_KERNEL);
	struct atomic_fixture *f = new_fixture(test);
	struct atomic_fixture *outputs[] = { f, other };
	struct drm_atomic_commit *state;
	struct drm_crtc_state *proposed;
	struct drm_modeset_acquire_ctx ctx;
	u64 generations[2];
	unsigned int i;
	int ret;

	KUNIT_ASSERT_NOT_NULL(test, other);
	init_additional_output(test, other, f->dev);
	state = new_update(test, f, f->target, f->tiled);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	ret = lock_update(state, &ctx);
	proposed = ret ? ERR_PTR(ret) : drm_atomic_get_crtc_state(state, other->crtc);
	if (!IS_ERR(proposed))
		ret = drm_atomic_set_constraints_for_crtc(proposed, other->target);
	unlock_update(state);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, proposed);
	KUNIT_ASSERT_EQ(test, ret, 0);
	KUNIT_ASSERT_EQ(test, run_update(state, check_update), 0);
	if (withdraw)
		KUNIT_ASSERT_EQ(test,
			drm_constraints_list_withdraw(drm_constraints_crtc_list(other->crtc),
						      drm_constraints_entry_id(other->target)), 0);
	other->backends[1].failed = fail;
	for (i = 0; i < ARRAY_SIZE(outputs); i++)
		KUNIT_ASSERT_EQ(test, drm_constraints_list_observe(
			drm_constraints_crtc_list(outputs[i]->crtc), &generations[i]), 0);
	KUNIT_EXPECT_EQ(test, run_update(state, install_update),
			withdraw ? -ESTALE : fail ? -EIO : 0);
	KUNIT_EXPECT_EQ(test, f->installs, withdraw || fail ? 0 : 1);
	for (i = 0; i < ARRAY_SIZE(outputs); i++) {
		struct atomic_fixture *output = outputs[i];
		struct drm_constraints_list *list = drm_constraints_crtc_list(output->crtc);
		struct drm_constraints_entry *selected = drm_constraints_list_selected(list);
		u64 generation;

		KUNIT_EXPECT_PTR_EQ(test, selected,
				    withdraw || fail ? output->initial : output->target);
		drm_constraints_entry_put(selected);
		KUNIT_ASSERT_EQ(test, drm_constraints_list_observe(list, &generation), 0);
		KUNIT_EXPECT_EQ(test, generation, generations[i] + !(withdraw || fail));
	}
}

static void constraints_cohort_selects_all_outputs_once(struct kunit *test)
{
	constraints_cohort_acceptance(test, false, false);
}

static void withdrawn_last_selection_rejects_whole_cohort(struct kunit *test)
{
	constraints_cohort_acceptance(test, true, false);
}

static void failed_last_backend_rejects_whole_cohort(struct kunit *test)
{
	constraints_cohort_acceptance(test, false, true);
}

static void multi_output_shutdown_rechecks_every_binding(struct kunit *test)
{
	struct atomic_fixture *other = kunit_kzalloc(test, sizeof(*other), GFP_KERNEL);
	struct atomic_fixture *f = new_fixture(test);
	struct drm_crtc_state *first_before, *other_before, *proposed;
	struct drm_atomic_commit *state;
	struct drm_modeset_acquire_ctx ctx;
	int ret;

	KUNIT_ASSERT_NOT_NULL(test, other);
	init_additional_output(test, other, f->dev);
	first_before = f->crtc->state;
	other_before = other->crtc->state;
	state = new_disable(test, f);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	ret = lock_update(state, &ctx);
	proposed = ret ? ERR_PTR(ret) : drm_atomic_get_crtc_state(state, other->crtc);
	unlock_update(state);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, proposed);
	KUNIT_ASSERT_EQ(test, run_update(state, drm_atomic_check_only), 0);
	/* Inject a changed target after validation; neither output may install. */
	drm_constraints_entry_put(proposed->constraints);
	proposed->constraints = drm_constraints_entry_get(other->target);
	other->backends[1].failed = true;
	KUNIT_EXPECT_EQ(test, run_update(state, swap_update), -EIO);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state, first_before);
	KUNIT_EXPECT_PTR_EQ(test, other->crtc->state, other_before);
	drm_constraints_entry_put(proposed->constraints);
	proposed->constraints = drm_constraints_entry_get(other->initial);
	drm_constraints_list_close(drm_constraints_crtc_list(f->crtc));
	drm_constraints_list_close(drm_constraints_crtc_list(other->crtc));
	KUNIT_ASSERT_EQ(test, run_update(state, swap_update), 0);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, f->initial);
	KUNIT_EXPECT_PTR_EQ(test, other->crtc->state->constraints, other->initial);
}

static int commit_prepared_update(struct drm_atomic_commit *state)
{
	struct drm_crtc *crtc = NULL;
	struct drm_crtc_state *proposed;
	struct drm_prepare_ticket *ticket;
	int i, ret = drm_atomic_check_only(state);

	if (ret)
		return ret;
	for_each_new_crtc_in_state(state, crtc, proposed, i)
		break;
	if (!crtc)
		return -EINVAL;
	ticket = drm_atomic_prepare_crtcs(&crtc, 1, NULL);
	if (IS_ERR(ticket))
		return PTR_ERR(ticket);
	ret = drm_atomic_commit_prepare(state, ticket, drm_atomic_prepare_display_observe);
	if (!ret)
		ret = commit_update(state->dev, state, false);
	drm_prepare_ticket_put(ticket);
	return ret;
}

struct shutdown_worker {
	struct drm_device *dev;
	struct completion finished;
	bool recover;
	int result;
};

static int shutdown_device(void *data)
{
	struct shutdown_worker *worker = data;

	if (worker->recover)
		worker->result = drm_constraints_recover(worker->dev);
	else
		drm_atomic_helper_shutdown(worker->dev);
	complete(&worker->finished);
	while (!kthread_should_stop())
		schedule_timeout_interruptible(1);
	return 0;
}

static void check_prepared_shutdown(struct kunit *test, bool recover)
{
	struct atomic_fixture *other = kunit_kzalloc(test, sizeof(*other), GFP_KERNEL);
	struct atomic_fixture *f = new_fixture_with_preparation(test, true);
	struct atomic_fixture *outputs[] = { f, other };
	struct shutdown_worker worker = { .dev = f->dev, .recover = recover };
	struct drm_prepare_source *source;
	struct drm_prepare_read_claim *read;
	struct dma_fence *fence;
	struct task_struct *task;
	struct completion installed;
	unsigned int i;
	bool early;

	KUNIT_ASSERT_NOT_NULL(test, other);
	init_additional_output(test, other, f->dev);
	for (i = 0; i < ARRAY_SIZE(outputs); i++) {
		struct atomic_fixture *output = outputs[i];
		struct drm_atomic_commit *state = new_update(test, output,
							    output->target, output->tiled);

		KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
		KUNIT_ASSERT_EQ(test, run_update(state, commit_prepared_update), 0);
		drm_atomic_commit_clear(state);
	}
	fence = kzalloc_obj(*fence);
	KUNIT_ASSERT_NOT_NULL(test, fence);
	dma_fence_init(fence, &read_fence_ops, NULL, dma_fence_context_alloc(1), 1);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, finish_read, fence), 0);
	KUNIT_ASSERT_EQ(test, drm_modeset_lock(&f->crtc->mutex, NULL), 0);
	source = drm_atomic_prepare_crtc_source(f->crtc);
	if (!IS_ERR(source))
		drm_prepare_source_get(source);
	drm_modeset_unlock(&f->crtc->mutex);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, source);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_source, source), 0);
	read = drm_prepare_source_claim(source);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, read);
	drm_prepare_read_release(read, fence);
	for (i = 0; i < ARRAY_SIZE(outputs); i++) {
		if (!recover)
			drm_constraints_list_close(drm_constraints_crtc_list(outputs[i]->crtc));
		outputs[i]->backends[1].failed = true;
	}
	init_completion(&worker.finished);
	init_completion(&installed);
	f->installed = &installed;
	task = kthread_run(shutdown_device, &worker, "constraints-shutdown");
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, task);
	KUNIT_EXPECT_NE(test, wait_for_completion_timeout(&installed, HZ), 0);
	early = wait_for_completion_timeout(&worker.finished, msecs_to_jiffies(20));
	KUNIT_EXPECT_FALSE(test, dma_fence_is_signaled(fence));
	read = drm_prepare_source_claim(source);
	KUNIT_EXPECT_TRUE(test, IS_ERR(read) && PTR_ERR(read) == -EBUSY);
	if (!IS_ERR(read))
		drm_prepare_read_release(read, NULL);
	dma_fence_signal(fence);
	kthread_stop(task);
	f->installed = NULL;
	KUNIT_EXPECT_FALSE(test, early);
	KUNIT_EXPECT_TRUE(test, completion_done(&worker.finished));
	KUNIT_EXPECT_EQ(test, worker.result, 0);
	KUNIT_EXPECT_EQ(test, f->installs, recover ? 5 : 3);
	for (i = 0; i < ARRAY_SIZE(outputs); i++) {
		KUNIT_EXPECT_FALSE(test, outputs[i]->crtc->state->enable);
		KUNIT_EXPECT_PTR_EQ(test, outputs[i]->plane->state->fb, NULL);
		KUNIT_EXPECT_PTR_EQ(test, outputs[i]->crtc->state->constraints,
				    recover ? outputs[i]->initial : outputs[i]->target);
	}
}

static void prepared_shutdown_retains_pending_native_reads(struct kunit *test)
{
	check_prepared_shutdown(test, false);
}

static void recovery_retains_pending_native_reads(struct kunit *test)
{
	check_prepared_shutdown(test, true);
}

static void master_replacement_waits_for_native_source_retirement(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture_with_preparation(test, true);
	struct drm_atomic_commit *state = new_update(test, f, f->target, f->tiled);
	struct drm_prepare_source *source;
	struct drm_prepare_read_claim *read;
	struct dma_fence *fence;
	struct file *file;
	bool acquired;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	KUNIT_ASSERT_EQ(test, run_update(state, commit_prepared_update), 0);
	drm_atomic_commit_clear(state);
	fence = kzalloc_obj(*fence);
	KUNIT_ASSERT_NOT_NULL(test, fence);
	dma_fence_init(fence, &read_fence_ops, NULL, dma_fence_context_alloc(1), 1);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, finish_read, fence), 0);
	KUNIT_ASSERT_EQ(test, drm_modeset_lock(&f->crtc->mutex, NULL), 0);
	source = drm_atomic_prepare_crtc_source(f->crtc);
	if (!IS_ERR(source))
		drm_prepare_source_get(source);
	drm_modeset_unlock(&f->crtc->mutex);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, source);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_source, source), 0);
	read = drm_prepare_source_claim(source);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, read);
	drm_prepare_read_release(read, fence);

	file = mock_drm_getfile(f->dev->primary, O_RDWR);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, file);
	atomic_inc(&f->dev->open_count);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, close_master_file, file), 0);
	KUNIT_ASSERT_EQ(test, drm_master_open(file->private_data), 0);
	KUNIT_ASSERT_EQ(test, drm_ioctl(file, DRM_IOCTL_DROP_MASTER, 0), 0L);
	KUNIT_EXPECT_EQ(test, drm_ioctl(file, DRM_IOCTL_SET_MASTER, 0), -EBUSY);
	acquired = drm_master_internal_acquire(f->dev);
	KUNIT_EXPECT_FALSE(test, acquired);
	if (acquired)
		drm_master_internal_release(f->dev);
	KUNIT_EXPECT_FALSE(test, dma_fence_is_signaled(fence));

	dma_fence_signal(fence);
	drm_constraints_owner_flush(f->dev);
	KUNIT_EXPECT_EQ(test, drm_ioctl(file, DRM_IOCTL_SET_MASTER, 0), 0L);
	KUNIT_EXPECT_FALSE(test, f->crtc->state->enable);
	KUNIT_EXPECT_PTR_EQ(test, f->plane->state->fb, NULL);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, f->initial);
}

static void proposed_scene_obeys_scalar_property_rules(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	const struct drm_constraints_property rules[] = {
		{ .object_id = f->plane->base.id, .property_id = f->plane->alpha_property->base.id,
		  .type = DRM_MODE_PROP_RANGE, .minimum = 32768, .maximum = 65535 },
		{ .object_id = f->plane->base.id,
		  .property_id = f->plane->rotation_property->base.id,
		  .type = DRM_MODE_PROP_BITMASK, .mask = DRM_MODE_ROTATE_0 },
		{ .object_id = f->plane->base.id, .property_id = f->plane->zpos_property->base.id,
		  .type = DRM_MODE_PROP_RANGE, .minimum = 1, .maximum = 2 },
		{ .object_id = f->plane->base.id,
		  .property_id = f->plane->color_encoding_property->base.id,
		  .type = DRM_MODE_PROP_ENUM, .mask = BIT_ULL(DRM_COLOR_YCBCR_BT709) },
		{ .object_id = f->crtc->base.id,
		  .property_id = f->dev->mode_config.prop_vrr_enabled->base.id,
		  .type = DRM_MODE_PROP_RANGE, .minimum = 0, .maximum = 0 },
		{ .object_id = f->plane->base.id,
		  .property_id = f->dev->mode_config.prop_crtc_x->base.id,
		  .type = DRM_MODE_PROP_SIGNED_RANGE, .minimum = (u64)-4, .maximum = 4 },
	};
	struct drm_constraints_entry *target = new_entry(test, f, DRM_FORMAT_ARGB8888,
					I915_FORMAT_MOD_X_TILED, 1, rules, ARRAY_SIZE(rules));
	struct drm_atomic_commit *state;
	struct drm_crtc_state *crtc;
	struct drm_plane_state *plane;

	KUNIT_ASSERT_EQ(test, drm_constraints_crtc_add(f->crtc, target), 0);
	state = new_update(test, f, target, f->tiled);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	plane = drm_atomic_get_new_plane_state(state, f->plane);
	crtc = drm_atomic_get_new_crtc_state(state, f->crtc);
	plane->zpos = 1;
	plane->color_encoding = DRM_COLOR_YCBCR_BT709;
	plane->alpha = 32767;
	KUNIT_EXPECT_EQ(test, run_update(state, drm_atomic_check_only), -EINVAL);
	plane->alpha = 32768;
	plane->rotation = DRM_MODE_ROTATE_90;
	KUNIT_EXPECT_EQ(test, run_update(state, drm_atomic_check_only), -EINVAL);
	plane->rotation = DRM_MODE_ROTATE_0;
	plane->zpos = 3;
	KUNIT_EXPECT_EQ(test, run_update(state, drm_atomic_check_only), -EINVAL);
	plane->zpos = 2;
	plane->color_encoding = DRM_COLOR_YCBCR_BT601;
	KUNIT_EXPECT_EQ(test, run_update(state, drm_atomic_check_only), -EINVAL);
	plane->color_encoding = DRM_COLOR_YCBCR_BT709;
	crtc->vrr_enabled = true;
	KUNIT_EXPECT_EQ(test, run_update(state, drm_atomic_check_only), -EINVAL);
	crtc->vrr_enabled = false;
	plane->crtc_x = -5;
	KUNIT_EXPECT_EQ(test, run_update(state, drm_atomic_check_only), -EINVAL);
	plane->crtc_x = -4;
	KUNIT_ASSERT_EQ(test, run_update(state, drm_atomic_check_only), 0);
	KUNIT_ASSERT_EQ(test, run_update(state, swap_update), 0);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, target);
	KUNIT_EXPECT_EQ(test, f->plane->state->alpha, 32768);
	KUNIT_EXPECT_EQ(test, f->plane->state->crtc_x, -4);
}

static void installation_rechecks_proposed_property_values(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	const struct drm_constraints_property rule = {
		.object_id = f->plane->base.id, .property_id = f->plane->alpha_property->base.id,
		.type = DRM_MODE_PROP_RANGE, .minimum = 32768, .maximum = 65535,
	};
	struct drm_constraints_entry *target = new_entry(test, f, DRM_FORMAT_ARGB8888,
							I915_FORMAT_MOD_X_TILED, 1, &rule, 1);
	struct drm_atomic_commit *state;
	struct drm_crtc_state *before = f->crtc->state;

	KUNIT_ASSERT_EQ(test, drm_constraints_crtc_add(f->crtc, target), 0);
	state = new_update(test, f, target, f->tiled);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	KUNIT_ASSERT_EQ(test, run_update(state, drm_atomic_check_only), 0);
	/* Inject a post-check driver mutation; installation must fail before swap. */
	drm_atomic_get_new_plane_state(state, f->plane)->alpha = 0;
	KUNIT_EXPECT_EQ(test, run_update(state, swap_update), -EINVAL);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state, before);
	KUNIT_EXPECT_PTR_EQ(test, f->plane->state->fb, NULL);
}

static void proposed_scene_obeys_plane_geometry_rules(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	const struct drm_constraints_size size = { 128, 64, 128, 64 };
	const struct drm_constraints_format allocation = {
		.plane_id = f->plane->base.id,
		.format = DRM_FORMAT_ARGB8888,
		.modifier = I915_FORMAT_MOD_X_TILED,
		.size = size,
		.storage_flags = DRM_CONSTRAINTS_FORMAT_STORAGE_NATIVE,
		.pitch_alignment = 1,
		.offset_alignment = 1,
		.max_pitch = U32_MAX,
	};
	const struct drm_constraints_plane_geometry geometry = {
		.plane_id = f->plane->base.id,
		.min_scale = 1 << 15,
		.max_scale = 1 << 17,
	};
	struct drm_constraints_description *description;
	struct drm_constraints_entry *entry;
	struct drm_atomic_commit *state;
	struct drm_plane_state *plane;

	description = drm_constraints_description_create_with_geometry(
		&size, &allocation, 1, NULL, 0, NULL, 0, &geometry, 1);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, description);
	KUNIT_ASSERT_EQ(test,
		kunit_add_action_or_reset(test, put_description, description), 0);
	entry = drm_constraints_entry_create(drm_constraints_device_domain(f->dev),
		f->crtc->base.id, description, &entry_ops, &f->backends[1]);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, entry);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_entry, entry), 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_crtc_add(f->crtc, entry), 0);
	state = new_update(test, f, entry, f->tiled);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	plane = drm_atomic_get_new_plane_state(state, f->plane);

	plane->crtc_x = 1;
	KUNIT_EXPECT_EQ(test, run_update(state, drm_atomic_check_only), -EINVAL);
	plane->crtc_x = 0;
	plane->src_x = 1 << 15;
	KUNIT_EXPECT_EQ(test, run_update(state, drm_atomic_check_only), -EINVAL);
	plane->src_x = 0;
	plane->src_w = 64 << 16;
	KUNIT_EXPECT_EQ(test, run_update(state, drm_atomic_check_only), -EINVAL);
	plane->src_w = 128 << 16;
	plane->crtc_w = 63;
	KUNIT_EXPECT_EQ(test, run_update(state, drm_atomic_check_only), -EINVAL);
	plane->crtc_w = 64;
	KUNIT_EXPECT_EQ(test, run_update(state, drm_atomic_check_only), 0);
	/* Acceptance must reject geometry changed after the ordinary check. */
	plane->crtc_w = 63;
	KUNIT_EXPECT_EQ(test, run_update(state, swap_update), -EINVAL);
	KUNIT_EXPECT_PTR_EQ(test, f->plane->state->fb, NULL);
}

static struct drm_plane *new_scene_plane(struct kunit *test, struct atomic_fixture *f,
					enum drm_plane_type type, unsigned int zpos)
{
	static const u32 formats[] = { DRM_FORMAT_ARGB8888 };
	static const u64 modifiers[] = { DRM_FORMAT_MOD_LINEAR, DRM_FORMAT_MOD_INVALID };
	struct drm_plane *plane;

	plane = __drmm_universal_plane_alloc(f->dev, sizeof(*plane), 0,
			drm_crtc_mask(f->crtc), f->plane->funcs, formats, ARRAY_SIZE(formats),
			modifiers, type, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, plane);
	drm_plane_helper_add(plane, f->plane->helper_private);
	KUNIT_ASSERT_EQ(test, drm_plane_create_alpha_property(plane), 0);
	KUNIT_ASSERT_EQ(test, drm_plane_create_zpos_property(plane, zpos, 0, 3), 0);
	return plane;
}

static struct drm_constraints_entry *
new_scene_entry(struct kunit *test, struct atomic_fixture *f,
		struct drm_plane *overlay, struct drm_plane *cursor,
		const struct drm_constraints_plane_limit *plane_limits,
		unsigned int plane_limit_count)
{
	const struct drm_constraints_size output = { 128, 64, 128, 64 };
	const struct drm_constraints_format formats[] = {
		{
			.plane_id = f->plane->base.id,
			.format = DRM_FORMAT_ARGB8888,
			.modifier = I915_FORMAT_MOD_X_TILED,
			.size = { 128, 64, 128, 64 },
			.storage_flags = DRM_CONSTRAINTS_FORMAT_STORAGE_NATIVE,
			.pitch_alignment = 1,
			.offset_alignment = 1,
			.max_pitch = U32_MAX,
		}, {
			.plane_id = overlay->base.id,
			.format = DRM_FORMAT_ARGB8888,
			.modifier = DRM_FORMAT_MOD_LINEAR,
			.size = { 128, 64, 128, 64 },
			.storage_flags = DRM_CONSTRAINTS_FORMAT_STORAGE_NATIVE,
			.pitch_alignment = 1,
			.offset_alignment = 1,
			.max_pitch = U32_MAX,
		}, {
			.plane_id = cursor->base.id,
			.format = DRM_FORMAT_ARGB8888,
			.modifier = DRM_FORMAT_MOD_LINEAR,
			.size = { 64, 64, 64, 64 },
			.storage_flags = DRM_CONSTRAINTS_FORMAT_STORAGE_NATIVE,
			.pitch_alignment = 1,
			.offset_alignment = 1,
			.max_pitch = U32_MAX,
		},
	};
	const struct drm_constraints_property rules[] = {
		{ .object_id = overlay->base.id, .property_id = overlay->alpha_property->base.id,
		  .type = DRM_MODE_PROP_RANGE, .minimum = 32768, .maximum = 65535 },
		{ .object_id = overlay->base.id, .property_id = overlay->zpos_property->base.id,
		  .type = DRM_MODE_PROP_RANGE, .minimum = 1, .maximum = 1 },
		{ .object_id = cursor->base.id, .property_id = cursor->zpos_property->base.id,
		  .type = DRM_MODE_PROP_RANGE, .minimum = 2, .maximum = 2 },
		{ .object_id = overlay->base.id,
		  .property_id = f->dev->mode_config.prop_src_w->base.id,
		  .type = DRM_MODE_PROP_RANGE, .minimum = 64 << 16, .maximum = 64 << 16 },
		{ .object_id = overlay->base.id,
		  .property_id = f->dev->mode_config.prop_crtc_w->base.id,
		  .type = DRM_MODE_PROP_RANGE, .minimum = 32, .maximum = 32 },
	};
	struct drm_constraints_description *description;
	struct drm_constraints_entry *entry;

	description = drm_constraints_description_create(
		&output, formats, ARRAY_SIZE(formats), rules, ARRAY_SIZE(rules),
		plane_limits, plane_limit_count);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, description);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_description, description), 0);
	entry = drm_constraints_entry_create(drm_constraints_device_domain(f->dev),
			f->crtc->base.id, description, &entry_ops, &f->backends[1]);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, entry);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_entry, entry), 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_crtc_add(f->crtc, entry), 0);
	return entry;
}

static int add_scene_plane(struct drm_atomic_commit *state, struct drm_crtc *crtc,
			   struct drm_plane *plane, struct drm_framebuffer *fb, bool crop)
{
	struct drm_modeset_acquire_ctx ctx;
	struct drm_plane_state *proposed;
	int ret = lock_update(state, &ctx);

	if (ret)
		goto out;
	proposed = drm_atomic_get_plane_state(state, plane);
	if (IS_ERR(proposed)) {
		ret = PTR_ERR(proposed);
		goto out;
	}
	ret = drm_atomic_set_crtc_for_plane(proposed, crtc);
	if (ret)
		goto out;
	drm_atomic_set_fb_for_plane(proposed, fb);
	proposed->src_x = crop ? 16 << 16 : 0;
	proposed->src_y = crop ? 16 << 16 : 0;
	proposed->src_w = 64 << 16;
	proposed->src_h = (crop ? 32 : 64) << 16;
	proposed->crtc_w = crop ? 32 : 64;
	proposed->crtc_h = crop ? 16 : 64;
out:
	unlock_update(state);
	return ret;
}

static int set_scene_plane(struct drm_atomic_commit *state, struct drm_crtc *crtc,
			   struct drm_plane *plane, struct drm_framebuffer *fb)
{
	struct drm_modeset_acquire_ctx ctx;
	struct drm_plane_state *proposed;
	int ret = lock_update(state, &ctx);

	if (ret)
		goto out;
	proposed = drm_atomic_get_plane_state(state, plane);
	if (IS_ERR(proposed)) {
		ret = PTR_ERR(proposed);
		goto out;
	}
	ret = drm_atomic_set_crtc_for_plane(proposed, crtc);
	if (!ret)
		drm_atomic_set_fb_for_plane(proposed, fb);
out:
	unlock_update(state);
	return ret;
}

static void complete_scene_checks_overlay_and_cursor_contracts(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_plane *overlay = new_scene_plane(test, f, DRM_PLANE_TYPE_OVERLAY, 1);
	struct drm_plane *cursor = new_scene_plane(test, f, DRM_PLANE_TYPE_CURSOR, 2);
	struct drm_framebuffer *overlay_fb = new_fb(test, f, DRM_FORMAT_ARGB8888,
							  DRM_FORMAT_MOD_LINEAR, 128);
	struct drm_framebuffer *cursor_fb = new_fb(test, f, DRM_FORMAT_ARGB8888,
							 DRM_FORMAT_MOD_LINEAR, 64);
	struct drm_constraints_entry *entry = new_scene_entry(test, f, overlay, cursor, NULL, 0);
	struct drm_atomic_commit *state, *next;
	struct drm_plane_state *plane;

	drm_mode_config_reset(f->dev);
	state = new_update(test, f, entry, f->tiled);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	KUNIT_ASSERT_EQ(test, add_scene_plane(state, f->crtc, overlay, overlay_fb, true), 0);
	KUNIT_ASSERT_EQ(test, add_scene_plane(state, f->crtc, cursor, cursor_fb, false), 0);
	plane = drm_atomic_get_new_plane_state(state, overlay);
	plane->alpha = 32767;
	KUNIT_EXPECT_EQ(test, run_update(state, drm_atomic_check_only), -EINVAL);
	plane->alpha = 32768;
	plane->src_w = 63 << 16;
	KUNIT_EXPECT_EQ(test, run_update(state, drm_atomic_check_only), -EINVAL);
	plane->src_w = 64 << 16;
	plane->crtc_w = 31;
	KUNIT_EXPECT_EQ(test, run_update(state, drm_atomic_check_only), -EINVAL);
	plane->crtc_w = 32;
	/* Equal dimensions and fourcc do not make a different modifier valid. */
	drm_atomic_set_fb_for_plane(plane, f->tiled);
	KUNIT_EXPECT_EQ(test, run_update(state, drm_atomic_check_only), -EINVAL);
	drm_atomic_set_fb_for_plane(plane, overlay_fb);
	plane = drm_atomic_get_new_plane_state(state, cursor);
	plane->zpos = 1;
	KUNIT_EXPECT_EQ(test, run_update(state, drm_atomic_check_only), -EINVAL);
	plane->zpos = 2;
	/* A cursor allocation cannot borrow the overlay's larger source bounds. */
	drm_atomic_set_fb_for_plane(plane, overlay_fb);
	KUNIT_EXPECT_EQ(test, run_update(state, drm_atomic_check_only), -EINVAL);
	drm_atomic_set_fb_for_plane(plane, cursor_fb);
	KUNIT_ASSERT_EQ(test, run_update(state, drm_atomic_check_only), 0);
	KUNIT_ASSERT_EQ(test, run_update(state, swap_update), 0);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, entry);
	KUNIT_EXPECT_PTR_EQ(test, overlay->state->fb, overlay_fb);
	KUNIT_EXPECT_PTR_EQ(test, cursor->state->fb, cursor_fb);
	next = new_update(test, f, f->target, f->tiled);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, next);
	KUNIT_EXPECT_PTR_EQ(test, drm_atomic_get_new_plane_state(next, overlay), NULL);
	KUNIT_EXPECT_PTR_EQ(test, drm_atomic_get_new_plane_state(next, cursor), NULL);
	/* A primary-only contract must not discard unchanged overlay/cursor state. */
	KUNIT_EXPECT_EQ(test, run_update(next, drm_atomic_check_only), -EINVAL);
	KUNIT_EXPECT_NOT_NULL(test, drm_atomic_get_new_plane_state(next, overlay));
	KUNIT_EXPECT_NOT_NULL(test, drm_atomic_get_new_plane_state(next, cursor));
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, entry);
}

static void proposed_scene_obeys_overlapping_plane_limits(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_plane *overlay = new_scene_plane(test, f, DRM_PLANE_TYPE_OVERLAY, 1);
	struct drm_plane *cursor = new_scene_plane(test, f, DRM_PLANE_TYPE_CURSOR, 2);
	struct drm_framebuffer *overlay_fb = new_fb(test, f, DRM_FORMAT_ARGB8888,
							  DRM_FORMAT_MOD_LINEAR, 128);
	struct drm_framebuffer *cursor_fb = new_fb(test, f, DRM_FORMAT_ARGB8888,
							 DRM_FORMAT_MOD_LINEAR, 64);
	const u32 all_planes[] = { f->plane->base.id, overlay->base.id, cursor->base.id };
	const u32 auxiliary_planes[] = { overlay->base.id, cursor->base.id };
	const struct drm_constraints_plane_limit limits[] = {
		{ .max_active = 2, .count = ARRAY_SIZE(all_planes), .plane_ids = all_planes },
		{ .max_active = 1, .count = ARRAY_SIZE(auxiliary_planes),
		  .plane_ids = auxiliary_planes },
	};
	struct drm_constraints_entry *entry = new_scene_entry(test, f, overlay, cursor,
							       limits, ARRAY_SIZE(limits));
	struct drm_atomic_commit *state;

	drm_mode_config_reset(f->dev);
	state = new_update(test, f, entry, f->tiled);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	KUNIT_ASSERT_EQ(test, add_scene_plane(state, f->crtc, overlay, overlay_fb, true), 0);
	KUNIT_ASSERT_EQ(test, add_scene_plane(state, f->crtc, cursor, cursor_fb, false), 0);
	KUNIT_EXPECT_EQ(test, run_update(state, drm_atomic_check_only), -EINVAL);

	KUNIT_ASSERT_EQ(test, set_scene_plane(state, NULL, f->plane, NULL), 0);
	/* Two active planes fit the broad limit but not the auxiliary limit. */
	KUNIT_EXPECT_EQ(test, run_update(state, drm_atomic_check_only), -EINVAL);

	KUNIT_ASSERT_EQ(test, set_scene_plane(state, f->crtc, f->plane, f->tiled), 0);
	KUNIT_ASSERT_EQ(test, set_scene_plane(state, NULL, cursor, NULL), 0);
	KUNIT_EXPECT_EQ(test, run_update(state, drm_atomic_check_only), 0);
}

static void put_lease_master(void *data)
{
	struct drm_master *master = data;

	drm_master_put(&master);
}

static struct drm_master *new_lease_root(struct kunit *test, struct drm_device *dev)
{
	struct drm_master *master = drm_master_create(dev);

	KUNIT_ASSERT_NOT_NULL(test, master);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_lease_master, master), 0);
	return master;
}

static struct drm_master *lease_outputs(struct kunit *test, struct drm_master *root,
					struct drm_crtc *first, struct drm_crtc *second,
					int expected)
{
	struct drm_master *lease;
	DEFINE_IDR(ids);
	int ret;

	ret = idr_alloc(&ids, first, first->base.id, first->base.id + 1, GFP_KERNEL);
	if (ret < 0)
		idr_destroy(&ids);
	KUNIT_ASSERT_EQ(test, ret, first->base.id);
	if (second) {
		ret = idr_alloc(&ids, second, second->base.id, second->base.id + 1, GFP_KERNEL);
		if (ret < 0)
			idr_destroy(&ids);
		KUNIT_ASSERT_EQ(test, ret, second->base.id);
	}
	lease = drm_lease_create(root, &ids);
	if (IS_ERR(lease)) {
		idr_destroy(&ids);
		KUNIT_EXPECT_EQ(test, PTR_ERR(lease), expected);
		return NULL;
	}
	KUNIT_EXPECT_EQ(test, expected, 0);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_lease_master, lease), 0);
	return lease;
}

static void leases_preserve_default_until_revocation(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_master *root = new_lease_root(test, f->dev);
	struct drm_master *lease = lease_outputs(test, root, f->crtc, NULL, 0);
	struct drm_atomic_commit *fixed = new_update(test, f, f->initial, f->linear);
	struct drm_atomic_commit *target = new_update(test, f, f->target, f->tiled);

	KUNIT_ASSERT_NOT_NULL(test, lease);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, fixed);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, target);
	KUNIT_EXPECT_EQ(test, run_update(fixed, check_update), 0);
	KUNIT_EXPECT_EQ(test, run_update(target, check_update), -EBUSY);
	drm_lease_revoke(lease);
	drm_lease_revoke(lease);
	KUNIT_EXPECT_EQ(test, run_update(target, check_update), 0);
}

static void installation_rechecks_newly_created_leases(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_master *root = new_lease_root(test, f->dev);
	struct drm_atomic_commit *state = new_update(test, f, f->target, f->tiled);
	struct drm_master *lease;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	KUNIT_ASSERT_EQ(test, run_update(state, drm_atomic_check_only), 0);
	lease = lease_outputs(test, root, f->crtc, NULL, 0);
	KUNIT_ASSERT_NOT_NULL(test, lease);
	KUNIT_EXPECT_EQ(test, run_update(state, swap_update), -EBUSY);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, f->initial);
	drm_lease_revoke(lease);
	KUNIT_ASSERT_EQ(test, run_update(state, swap_update), 0);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, f->target);
}

static void nondefault_selection_cannot_be_inherited_by_a_lease(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_master *root = new_lease_root(test, f->dev);
	struct drm_atomic_commit *state = new_update(test, f, f->target, f->tiled);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	KUNIT_ASSERT_EQ(test, run_update(state, drm_atomic_check_only), 0);
	KUNIT_ASSERT_EQ(test, run_update(state, swap_update), 0);
	KUNIT_EXPECT_PTR_EQ(test, lease_outputs(test, root, f->crtc, NULL, -EBUSY), NULL);
	KUNIT_EXPECT_TRUE(test, list_empty(&root->lessees));
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, f->target);
}

static void each_master_tree_retains_its_own_leased_contract(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_master *root = new_lease_root(test, f->dev);
	struct drm_master *other = new_lease_root(test, f->dev);
	struct drm_master *first = lease_outputs(test, root, f->crtc, NULL, 0);
	struct drm_master *second = lease_outputs(test, other, f->crtc, NULL, 0);
	struct drm_atomic_commit *state = new_update(test, f, f->target, f->tiled);

	KUNIT_ASSERT_NOT_NULL(test, first);
	KUNIT_ASSERT_NOT_NULL(test, second);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	KUNIT_EXPECT_EQ(test, run_update(state, check_update), -EBUSY);
	drm_lease_revoke(first);
	kunit_release_action(test, put_lease_master, first);
	KUNIT_EXPECT_EQ(test, run_update(state, check_update), -EBUSY);
	/* Destruction must return the charge even without explicit revocation. */
	kunit_release_action(test, put_lease_master, second);
	KUNIT_EXPECT_EQ(test, run_update(state, check_update), 0);
}

static void lease_failure_changes_no_other_output_contract(struct kunit *test)
{
	struct atomic_fixture *other = kunit_kzalloc(test, sizeof(*other), GFP_KERNEL);
	struct atomic_fixture *f = new_fixture(test);
	struct drm_master *root = new_lease_root(test, f->dev);
	struct drm_atomic_commit *selected, *candidate;

	KUNIT_ASSERT_NOT_NULL(test, other);
	init_additional_output(test, other, f->dev);
	selected = new_update(test, other, other->target, other->tiled);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, selected);
	KUNIT_ASSERT_EQ(test, run_update(selected, drm_atomic_check_only), 0);
	KUNIT_ASSERT_EQ(test, run_update(selected, swap_update), 0);
	KUNIT_EXPECT_PTR_EQ(test, lease_outputs(test, root, f->crtc, other->crtc, -EBUSY), NULL);
	candidate = new_update(test, f, f->target, f->tiled);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, candidate);
	KUNIT_EXPECT_EQ(test, run_update(candidate, check_update), 0);
}

static void an_unleased_output_can_select_other_constraints(struct kunit *test)
{
	struct atomic_fixture *other = kunit_kzalloc(test, sizeof(*other), GFP_KERNEL);
	struct atomic_fixture *f = new_fixture(test);
	struct drm_master *root = new_lease_root(test, f->dev);
	struct drm_atomic_commit *state;

	KUNIT_ASSERT_NOT_NULL(test, other);
	init_additional_output(test, other, f->dev);
	KUNIT_ASSERT_NOT_NULL(test, lease_outputs(test, root, f->crtc, NULL, 0));
	state = new_update(test, other, other->target, other->tiled);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	KUNIT_ASSERT_EQ(test, run_update(state, drm_atomic_check_only), 0);
	KUNIT_ASSERT_EQ(test, run_update(state, swap_update), 0);
	KUNIT_EXPECT_PTR_EQ(test, other->crtc->state->constraints, other->target);
}

static int set_constraints_property(struct drm_atomic_commit *state,
				    struct drm_file *file, u64 id)
{
	struct atomic_fixture *f = state->dev->dev_private;
	struct drm_modeset_acquire_ctx ctx;
	int ret = lock_update(state, &ctx);

	if (!ret)
		ret = drm_atomic_set_property(state, file, &f->crtc->base,
					      f->dev->mode_config.prop_constraints_id, id, false);
	unlock_update(state);
	return ret;
}

static void persistent_property_checks_opt_in_and_selection(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_property *property = f->dev->mode_config.prop_constraints_id;
	struct drm_file *file = kunit_kzalloc(test, sizeof(*file), GFP_KERNEL);
	struct drm_atomic_commit *state = new_update(test, f, NULL, f->tiled);
	u64 value = 0, id = drm_constraints_entry_id(f->target);

	KUNIT_ASSERT_NOT_NULL(test, file);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	KUNIT_EXPECT_STREQ(test, property->name, DRM_CONSTRAINTS_ID_PROPERTY);
	KUNIT_EXPECT_PTR_EQ(test, drm_mode_obj_find_prop_id(&f->crtc->base,
							   property->base.id), property);
	KUNIT_EXPECT_PTR_EQ(test, drm_mode_obj_find_prop_id(&f->plane->base,
							   property->base.id), NULL);
	drm_modeset_lock(&f->crtc->mutex, NULL);
	KUNIT_EXPECT_EQ(test, drm_atomic_get_property(&f->crtc->base, property, &value), 0);
	drm_modeset_unlock(&f->crtc->mutex);
	KUNIT_EXPECT_EQ(test, value, drm_constraints_entry_id(f->initial));
	KUNIT_EXPECT_EQ(test, set_constraints_property(state, file, id), -EOPNOTSUPP);
	file->kms_constraints = true;
	KUNIT_EXPECT_EQ(test, set_constraints_property(state, file, 0), -EINVAL);
	KUNIT_EXPECT_EQ(test, set_constraints_property(state, file, U64_MAX), -ESTALE);
	KUNIT_ASSERT_EQ(test, set_constraints_property(state, file, id), 0);
	state->allow_modeset = false;
	KUNIT_EXPECT_EQ(test, run_update(state, drm_atomic_check_only), -EINVAL);
	state->allow_modeset = true;
	KUNIT_ASSERT_EQ(test, run_update(state, drm_atomic_check_only), 0);
	drm_modeset_lock(&f->crtc->mutex, NULL);
	KUNIT_EXPECT_EQ(test, drm_atomic_get_property(&f->crtc->base, property, &value), 0);
	drm_modeset_unlock(&f->crtc->mutex);
	KUNIT_EXPECT_EQ(test, value, drm_constraints_entry_id(f->initial));
	KUNIT_ASSERT_EQ(test, run_update(state, swap_update), 0);
	drm_modeset_lock(&f->crtc->mutex, NULL);
	KUNIT_EXPECT_EQ(test, drm_atomic_get_property(&f->crtc->base, property, &value), 0);
	drm_modeset_unlock(&f->crtc->mutex);
	KUNIT_EXPECT_EQ(test, value, id);
}

static void persistent_property_can_be_repeated_during_closed_output_shutdown(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_atomic_commit *state = new_update(test, f, f->target, f->tiled);
	struct drm_atomic_commit *stop;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	KUNIT_ASSERT_EQ(test, run_update(state, drm_atomic_check_only), 0);
	KUNIT_ASSERT_EQ(test, run_update(state, swap_update), 0);
	drm_constraints_list_close(drm_constraints_crtc_list(f->crtc));
	stop = new_disable(test, f);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, stop);
	KUNIT_ASSERT_EQ(test, set_constraints_property(stop, NULL,
						      drm_constraints_entry_id(f->target)), 0);
	KUNIT_EXPECT_EQ(test,
		set_constraints_property(stop, NULL, drm_constraints_entry_id(f->initial)),
		-ESTALE);
	KUNIT_ASSERT_EQ(test, run_update(stop, drm_atomic_check_only), 0);
	KUNIT_ASSERT_EQ(test, run_update(stop, swap_update), 0);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, f->target);
	KUNIT_EXPECT_FALSE(test, f->crtc->state->enable);
}

static int allow_retained_request(struct drm_atomic_commit *state,
				  const struct drm_atomic_request *request, void *data)
{
	return 0;
}

static void constraints_requests_retain_identity_not_availability(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_atomic_commit *state = new_update(test, f, NULL, f->tiled);
	struct drm_constraints_list *list = drm_constraints_crtc_list(f->crtc);
	struct drm_atomic_request_entry value = { .scalar = 23 };
	struct drm_atomic_request *request;
	struct drm_modeset_acquire_ctx ctx;
	u64 id = drm_constraints_entry_id(f->target);
	int ret;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, state);
	KUNIT_ASSERT_EQ(test, lock_update(state, &ctx), 0);
	ret = drm_atomic_resolve_user_value(&f->crtc->base,
			f->dev->mode_config.prop_constraints_id, NULL, id, &value);
	unlock_update(state);
	KUNIT_ASSERT_EQ(test, ret, 0);
	KUNIT_EXPECT_EQ(test, value.type, DRM_ATOMIC_REQUEST_CONSTRAINTS);
	KUNIT_EXPECT_PTR_EQ(test, value.constraints, f->target);
	request = drm_atomic_request_create(f->dev, &value, 1);
	drm_atomic_release_user_value(&value);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, request);
	KUNIT_ASSERT_EQ(test, drm_constraints_list_withdraw(list, id), 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_list_forget(list, id), 0);
	kunit_release_action(test, put_entry, f->target);
	KUNIT_EXPECT_EQ(test, f->backends[1].released, 0);
	KUNIT_EXPECT_PTR_EQ(test, drm_atomic_request_entry(request, 0)->constraints, f->target);
	KUNIT_ASSERT_EQ(test, lock_update(state, &ctx), 0);
	ret = drm_atomic_request_apply(request, state, allow_retained_request, NULL);
	unlock_update(state);
	KUNIT_EXPECT_EQ(test, ret, -ESTALE);
	drm_atomic_request_destroy(request);
	KUNIT_EXPECT_EQ(test, f->backends[1].released, 1);
}

static void constraints_requests_reject_unretained_identifiers(struct kunit *test)
{
	struct atomic_fixture *f = new_fixture(test);
	struct drm_atomic_request_entry value = {
		.object = &f->crtc->base,
		.property = f->dev->mode_config.prop_constraints_id,
		.type = DRM_ATOMIC_REQUEST_SCALAR,
		.scalar = drm_constraints_entry_id(f->target),
	};
	struct drm_atomic_request *request = drm_atomic_request_create(f->dev, &value, 1);

	KUNIT_EXPECT_EQ(test, PTR_ERR(request), -EINVAL);
	value.type = DRM_ATOMIC_REQUEST_CONSTRAINTS;
	value.constraints = NULL;
	request = drm_atomic_request_create(f->dev, &value, 1);
	KUNIT_EXPECT_EQ(test, PTR_ERR(request), -EINVAL);
}

static struct kunit_case drm_constraints_atomic_tests[] = {
	KUNIT_CASE(persistent_property_checks_opt_in_and_selection),
	KUNIT_CASE(persistent_property_can_be_repeated_during_closed_output_shutdown),
	KUNIT_CASE(constraints_requests_retain_identity_not_availability),
	KUNIT_CASE(constraints_requests_reject_unretained_identifiers),
	KUNIT_CASE(leases_preserve_default_until_revocation),
	KUNIT_CASE(installation_rechecks_newly_created_leases),
	KUNIT_CASE(nondefault_selection_cannot_be_inherited_by_a_lease),
	KUNIT_CASE(each_master_tree_retains_its_own_leased_contract),
	KUNIT_CASE(lease_failure_changes_no_other_output_contract),
	KUNIT_CASE(an_unleased_output_can_select_other_constraints),
	KUNIT_CASE(target_creation_precedes_atomic_selection),
	KUNIT_CASE(atomic_layout_matching_distinguishes_implicit_from_linear),
	KUNIT_CASE(readiness_loss_after_check_prevents_installation),
	KUNIT_CASE(withdrawal_after_check_prevents_installation),
	KUNIT_CASE(selection_requires_modeset_permission),
	KUNIT_CASE(source_allocation_respects_exact_geometry),
	KUNIT_CASE(source_allocation_respects_storage_requirements),
	KUNIT_CASE(asynchronous_updates_are_not_admitted),
	KUNIT_CASE(transactions_may_include_outputs_without_constraints),
	KUNIT_CASE(validation_includes_unchanged_active_planes),
	KUNIT_CASE(selection_requires_owned_mutable_proposed_state),
	KUNIT_CASE(core_validation_observes_selected_constraints),
	KUNIT_CASE(state_swap_accepts_retained_backend),
	KUNIT_CASE(state_swap_rechecks_withdrawn_target),
	KUNIT_CASE(state_swap_rechecks_failed_backend),
	KUNIT_CASE(accepted_selection_persists_without_reselection),
	KUNIT_CASE(closed_failed_backend_can_be_disabled),
	KUNIT_CASE(closure_after_check_still_permits_disable),
	KUNIT_CASE(framebuffer_removal_preserves_accepted_binding),
	KUNIT_CASE(framebuffer_removal_can_disable_unavailable_output),
	KUNIT_CASE(default_restoration_requires_quiescent_output),
	KUNIT_CASE(default_restoration_rechecks_default_availability),
	KUNIT_CASE(closed_default_is_not_successful_restoration),
	KUNIT_CASE(recovery_restores_all_defaults_before_retiring_offers),
	KUNIT_CASE(recovery_does_not_reopen_closed_lists),
	KUNIT_CASE(recovery_retains_pending_native_reads),
	KUNIT_CASE(master_replacement_waits_for_native_source_retirement),
	KUNIT_CASE(owner_recovery_excludes_replacement_until_success),
	KUNIT_CASE(owner_recovery_stops_after_unplug_or_cleanup),
	KUNIT_CASE(closure_rejects_checked_activation),
	KUNIT_CASE(shutdown_cannot_select_through_closed_list),
	KUNIT_CASE(predecessor_backend_survives_native_read),
	KUNIT_CASE(failed_read_retires_without_rolling_back_target),
	KUNIT_CASE(closed_output_shutdown_waits_for_native_read),
	KUNIT_CASE(native_read_retirement_does_not_stall_another_output),
	KUNIT_CASE(independent_outputs_keep_exact_bindings_during_animation),
	KUNIT_CASE(shutdown_disables_all_unavailable_outputs),
	KUNIT_CASE(multi_output_shutdown_rechecks_every_binding),
	KUNIT_CASE(constraints_cohort_selects_all_outputs_once),
	KUNIT_CASE(withdrawn_last_selection_rejects_whole_cohort),
	KUNIT_CASE(failed_last_backend_rejects_whole_cohort),
	KUNIT_CASE(prepared_shutdown_retains_pending_native_reads),
	KUNIT_CASE(proposed_scene_obeys_scalar_property_rules),
	KUNIT_CASE(installation_rechecks_proposed_property_values),
	KUNIT_CASE(proposed_scene_obeys_plane_geometry_rules),
	KUNIT_CASE(complete_scene_checks_overlay_and_cursor_contracts),
	KUNIT_CASE(proposed_scene_obeys_overlapping_plane_limits),
	{}
};

static struct kunit_suite drm_constraints_atomic_test_suite = {
	.name = "drm_constraints_atomic",
	.test_cases = drm_constraints_atomic_tests,
};

kunit_test_suite(drm_constraints_atomic_test_suite);

MODULE_DESCRIPTION("DRM constraints atomic acceptance and retirement tests");
MODULE_LICENSE("Dual MIT/GPL");
