// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/completion.h>
#include <linux/file.h>
#include <linux/kthread.h>
#include <drm/drm_atomic.h>
#include <drm/drm_atomic_helper.h>
#include <drm/drm_atomic_prepare.h>
#include <drm/drm_atomic_prepare_auth.h>
#include <drm/drm_atomic_prepare_display.h>
#include <drm/drm_atomic_prepare_owner.h>
#include <drm/drm_atomic_uapi.h>
#include <drm/drm_auth.h>
#include <drm/drm_connector.h>
#include <drm/drm_file.h>
#include <drm/drm_fourcc.h>
#include <drm/drm_framebuffer.h>
#include <drm/drm_ioctl.h>
#include <drm/drm_kunit_helpers.h>
#include <drm/drm_plane.h>
#include <kunit/test.h>

#include "../drm_crtc_internal.h"

struct plane_fixture {
	struct drm_device *dev;
	struct drm_plane *plane;
	struct drm_crtc *crtc;
	struct drm_connector connector;
	struct drm_framebuffer *fb;
	struct file *file;
	struct drm_prepare_source *source;
	struct drm_prepare_read_claim *read;
	struct task_struct *worker;
	struct completion checked;
	unsigned int checks, installs, validations;
	int worker_error;
	bool drop_master, change_alpha;
};

static int check_update(struct drm_device *dev, struct drm_atomic_commit *state)
{
	struct plane_fixture *f = dev->dev_private;
	int ret = drm_atomic_helper_check(dev, state);

	f->checks++;
	complete_all(&f->checked);
	return ret;
}

static int install_update(struct drm_device *dev, struct drm_atomic_commit *state, bool nonblock)
{
	struct plane_fixture *f = dev->dev_private;
	int ret = drm_atomic_helper_swap_state(state, false);

	if (!ret)
		f->installs++;
	return ret;
}

static const struct drm_mode_config_funcs config_funcs = {
	.atomic_check = check_update,
	.atomic_commit = install_update,
};

static const struct drm_plane_funcs plane_funcs = {
	.reset = drm_atomic_helper_plane_reset,
	.atomic_duplicate_state = drm_atomic_helper_plane_duplicate_state,
	.atomic_destroy_state = drm_atomic_helper_plane_destroy_state,
	.update_plane_request = drm_atomic_helper_update_plane_request,
};

static const struct drm_connector_funcs connector_funcs = {
	.reset = drm_atomic_helper_connector_reset,
	.atomic_duplicate_state = drm_atomic_helper_connector_duplicate_state,
	.atomic_destroy_state = drm_atomic_helper_connector_destroy_state,
};

static const struct file_operations file_ops = {
	.owner = THIS_MODULE,
	.release = drm_release_noglobal,
};

static const struct drm_driver driver = {
	.driver_features = DRIVER_MODESET | DRIVER_ATOMIC,
	.fops = &file_ops,
};

static void destroy_fb(struct drm_framebuffer *fb)
{
	drm_framebuffer_cleanup(fb);
	kfree(fb);
}

static const struct drm_framebuffer_funcs fb_funcs = {
	.destroy = destroy_fb,
};

static void close_file(void *data)
{
	__fput_sync(data);
}

static void finish_fixture(void *data)
{
	struct plane_fixture *f = data;

	complete_all(&f->checked);
	if (f->worker)
		kthread_stop(f->worker);
	if (f->read)
		drm_prepare_read_abandon(f->read);
	if (f->source)
		drm_prepare_source_put(f->source);
	drm_framebuffer_put(f->fb);
}

static struct plane_fixture *new_fixture(struct kunit *test)
{
	const struct drm_display_mode mode = {
		DRM_MODE("64x64", 0, 1000, 64, 65, 66, 67, 0, 64, 65, 66, 67, 0, 0)
	};
	struct plane_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct device *parent = drm_kunit_helper_alloc_device(test);
	struct drm_framebuffer *fb;
	struct drm_master *master;
	struct drm_file *file;
	int ret;

	KUNIT_ASSERT_NOT_NULL(test, f);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	f->dev = __drm_kunit_helper_alloc_drm_device_with_driver(test, parent,
							   sizeof(*f->dev), 0, &driver);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->dev);
	f->dev->dev_private = f;
	f->dev->mode_config.funcs = &config_funcs;
	KUNIT_ASSERT_EQ(test, drm_atomic_prepare_display_init(f->dev, 8), 0);
	f->plane = drm_kunit_helper_create_primary_plane(test, f->dev, &plane_funcs,
							 NULL, NULL, 0, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->plane);
	f->crtc = drm_kunit_helper_create_crtc(test, f->dev, f->plane, NULL, NULL, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->crtc);
	KUNIT_ASSERT_EQ(test, drmm_connector_init(f->dev, &f->connector, &connector_funcs,
						DRM_MODE_CONNECTOR_VIRTUAL, NULL), 0);
	drm_mode_config_reset(f->dev);
	f->file = mock_drm_getfile(f->dev->primary, O_RDWR);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->file);
	atomic_inc(&f->dev->open_count);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, close_file, f->file), 0);
	file = f->file->private_data;
	master = kzalloc_obj(*master);
	KUNIT_ASSERT_NOT_NULL(test, master);
	kref_init(&master->refcount);
	master->dev = f->dev;
	idr_init_base(&master->magic_map, 1);
	idr_init(&master->leases);
	idr_init_base(&master->lessee_idr, 1);
	INIT_LIST_HEAD(&master->lessees);
	INIT_LIST_HEAD(&master->lessee_list);
	file->master = master;
	file->is_master = file->was_master = true;
	mutex_lock(&f->dev->master_mutex);
	f->dev->master = drm_master_get(master);
	mutex_unlock(&f->dev->master_mutex);
	fb = kzalloc_obj(*fb);
	KUNIT_ASSERT_NOT_NULL(test, fb);
	fb->dev = f->dev;
	fb->width = fb->height = 64;
	fb->pitches[0] = 256;
	fb->format = drm_format_info(DRM_FORMAT_XRGB8888);
	ret = drm_framebuffer_init(f->dev, fb, &fb_funcs);
	if (ret)
		kfree(fb);
	KUNIT_ASSERT_EQ(test, ret, 0);
	f->fb = fb;
	init_completion(&f->checked);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, finish_fixture, f), 0);
	ret = drm_modeset_lock(&f->crtc->mutex, NULL);
	KUNIT_ASSERT_EQ(test, ret, 0);
	ret = drm_atomic_set_mode_for_crtc(f->crtc->state, &mode);
	f->crtc->state->active = true;
	f->crtc->state->plane_mask = drm_plane_mask(f->plane);
	f->crtc->state->connector_mask = drm_connector_mask(&f->connector);
	drm_connector_get(&f->connector);
	f->connector.state->crtc = f->crtc;
	f->plane->state->crtc = f->crtc;
	drm_framebuffer_assign(&f->plane->state->fb, fb);
	f->plane->state->crtc_w = f->plane->state->crtc_h = 64;
	f->plane->state->src_w = f->plane->state->src_h = 64 << 16;
	drm_modeset_unlock(&f->crtc->mutex);
	KUNIT_ASSERT_EQ(test, ret, 0);
	return f;
}

static int finish_reader(void *data)
{
	struct plane_fixture *f = data;

	wait_for_completion(&f->checked);
	f->worker_error = drm_modeset_lock(&f->crtc->mutex, NULL);
	if (!f->worker_error)
		drm_modeset_unlock(&f->crtc->mutex);
	if (f->change_alpha) {
		int ret = drm_modeset_lock(&f->plane->mutex, NULL);

		if (!ret) {
			f->plane->state->alpha = 0x1234;
			drm_modeset_unlock(&f->plane->mutex);
		}
		if (!f->worker_error)
			f->worker_error = ret;
	}
	if (f->drop_master) {
		int ret = drm_ioctl(f->file, DRM_IOCTL_DROP_MASTER, 0);

		if (!f->worker_error)
			f->worker_error = ret;
	} else {
		drm_prepare_read_release(f->read, NULL);
		f->read = NULL;
	}
	set_current_state(TASK_INTERRUPTIBLE);
	while (!kthread_should_stop()) {
		schedule();
		set_current_state(TASK_INTERRUPTIBLE);
	}
	__set_current_state(TASK_RUNNING);
	return 0;
}

static void start_reader(struct kunit *test, struct plane_fixture *f)
{
	struct drm_prepare_source *source;
	struct drm_prepare_read_claim *read = NULL;
	int ret = drm_modeset_lock(&f->crtc->mutex, NULL);

	KUNIT_ASSERT_EQ(test, ret, 0);
	source = drm_atomic_prepare_crtc_source(f->crtc);
	if (!IS_ERR(source)) {
		f->source = drm_prepare_source_get(source);
		read = drm_prepare_source_claim(source);
	}
	drm_modeset_unlock(&f->crtc->mutex);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, source);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, read);
	f->read = read;
	f->worker = kthread_run(finish_reader, f, "drm-setplane-reader");
	if (IS_ERR(f->worker)) {
		ret = PTR_ERR(f->worker);
		f->worker = NULL;
	}
	KUNIT_ASSERT_EQ(test, ret, 0);
}

static void join_reader(struct plane_fixture *f)
{
	kthread_stop(f->worker);
	f->worker = NULL;
}

static void setplane_disable_waits_for_reader(struct kunit *test)
{
	struct plane_fixture *f = new_fixture(test);
	struct drm_mode_set_plane request = { .plane_id = f->plane->base.id };

	start_reader(test, f);
	KUNIT_EXPECT_EQ(test, drm_mode_setplane(f->dev, &request, f->file->private_data), 0);
	join_reader(f);
	KUNIT_EXPECT_EQ(test, f->worker_error, 0);
	KUNIT_EXPECT_GE(test, f->checks, 2);
	KUNIT_EXPECT_EQ(test, f->installs, 1);
	KUNIT_EXPECT_PTR_EQ(test, f->plane->state->fb, NULL);
	KUNIT_EXPECT_PTR_EQ(test, f->plane->state->crtc, NULL);
	KUNIT_EXPECT_EQ(test, f->plane->state->src_w, 0);
}

static void master_loss_cancels_setplane(struct kunit *test)
{
	struct plane_fixture *f = new_fixture(test);
	struct drm_mode_set_plane request = { .plane_id = f->plane->base.id };
	struct drm_plane_state *before = f->plane->state;

	f->drop_master = true;
	start_reader(test, f);
	KUNIT_EXPECT_EQ(test, drm_mode_setplane(f->dev, &request, f->file->private_data), -ECANCELED);
	join_reader(f);
	KUNIT_EXPECT_EQ(test, f->worker_error, 0);
	KUNIT_EXPECT_EQ(test, f->installs, 0);
	KUNIT_EXPECT_PTR_EQ(test, f->plane->state, before);
}

static void setplane_requires_request_callback(struct kunit *test)
{
	static const struct drm_plane_funcs unsupported = {
		.reset = drm_atomic_helper_plane_reset,
		.atomic_duplicate_state = drm_atomic_helper_plane_duplicate_state,
		.atomic_destroy_state = drm_atomic_helper_plane_destroy_state,
	};
	struct plane_fixture *f = new_fixture(test);
	struct drm_mode_set_plane request = { .plane_id = f->plane->base.id };

	f->plane->funcs = &unsupported;
	KUNIT_EXPECT_EQ(test, drm_mode_setplane(f->dev, &request, f->file->private_data), -EOPNOTSUPP);
	KUNIT_EXPECT_EQ(test, f->checks, 0);
	KUNIT_EXPECT_EQ(test, f->installs, 0);
}

static int validate_update(const struct drm_plane_update *update, void *data)
{
	struct plane_fixture *f = data;

	return ++f->validations == 1 ? 0 : -EACCES;
}

static void put_owner(void *data)
{
	drm_prepare_owner_put(data);
}

static void plane_request_revalidates_after_wait(struct kunit *test)
{
	struct plane_fixture *f = new_fixture(test);
	struct drm_plane_update update = { .plane = f->plane };
	struct drm_prepare_owner *owner = drm_file_prepare_owner(f->file->private_data);
	int ret;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, owner);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_owner, owner), 0);
	start_reader(test, f);
	ret = drm_atomic_helper_update_plane_request(&update, owner, validate_update, f);
	join_reader(f);
	KUNIT_EXPECT_EQ(test, ret, -EACCES);
	KUNIT_EXPECT_EQ(test, f->worker_error, 0);
	KUNIT_EXPECT_EQ(test, f->validations, 2);
	KUNIT_EXPECT_EQ(test, f->installs, 0);
}

static void plane_update_preserves_unrequested_current_state(struct kunit *test)
{
	struct plane_fixture *f = new_fixture(test);
	struct drm_plane_update update = {
		.plane = f->plane, .crtc = f->crtc, .fb = f->fb,
		.crtc_w = 32, .crtc_h = 32, .src_w = 32 << 16, .src_h = 32 << 16,
	};
	struct drm_prepare_owner *owner = drm_file_prepare_owner(f->file->private_data);
	int ret;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, owner);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_owner, owner), 0);
	f->change_alpha = true;
	start_reader(test, f);
	ret = drm_atomic_helper_update_plane_request(&update, owner, NULL, NULL);
	join_reader(f);
	KUNIT_EXPECT_EQ(test, ret, 0);
	KUNIT_EXPECT_EQ(test, f->worker_error, 0);
	KUNIT_EXPECT_GE(test, f->checks, 2);
	KUNIT_EXPECT_EQ(test, f->installs, 1);
	KUNIT_EXPECT_PTR_EQ(test, f->plane->state->fb, f->fb);
	KUNIT_EXPECT_EQ(test, f->plane->state->crtc_w, 32);
	KUNIT_EXPECT_EQ(test, f->plane->state->src_w, 32 << 16);
	KUNIT_EXPECT_EQ(test, f->plane->state->alpha, 0x1234);
}

static void plane_update_rejects_source_outside_framebuffer(struct kunit *test)
{
	struct plane_fixture *f = new_fixture(test);
	struct drm_plane_update update = {
		.plane = f->plane, .crtc = f->crtc, .fb = f->fb,
		.crtc_w = 64, .crtc_h = 64, .src_w = 65 << 16, .src_h = 64 << 16,
	};
	struct drm_prepare_owner *owner = drm_file_prepare_owner(f->file->private_data);
	int ret;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, owner);
	ret = drm_atomic_helper_update_plane_request(&update, owner, NULL, NULL);
	drm_prepare_owner_put(owner);
	KUNIT_EXPECT_EQ(test, ret, -ENOSPC);
	KUNIT_EXPECT_EQ(test, f->installs, 0);
	KUNIT_EXPECT_EQ(test, f->plane->state->src_w, 64 << 16);
}

static struct kunit_case cases[] = {
	KUNIT_CASE(setplane_disable_waits_for_reader),
	KUNIT_CASE(master_loss_cancels_setplane),
	KUNIT_CASE(setplane_requires_request_callback),
	KUNIT_CASE(plane_request_revalidates_after_wait),
	KUNIT_CASE(plane_update_preserves_unrequested_current_state),
	KUNIT_CASE(plane_update_rejects_source_outside_framebuffer),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_prepare_setplane",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_LICENSE("GPL");
MODULE_IMPORT_NS("EXPORTED_FOR_KUNIT_TESTING");
