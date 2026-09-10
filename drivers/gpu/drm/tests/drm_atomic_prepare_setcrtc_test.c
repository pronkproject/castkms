// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <drm/drm_atomic.h>
#include <drm/drm_atomic_helper.h>
#include <drm/drm_atomic_prepare.h>
#include <drm/drm_atomic_prepare_display.h>
#include <drm/drm_auth.h>
#include <drm/drm_file.h>
#include <drm/drm_ioctl.h>
#include <drm/drm_kunit_helpers.h>
#include <kunit/test.h>
#include <linux/completion.h>
#include <linux/file.h>
#include <linux/kthread.h>

#include "../drm_crtc_internal.h"

struct setcrtc_fixture {
	struct drm_device *dev;
	struct drm_crtc *crtc;
	struct file *file;
	struct drm_prepare_source *source;
	struct drm_prepare_read_claim *read;
	struct task_struct *worker;
	struct completion checked;
	unsigned int checks;
	unsigned int installs;
	int worker_error;
	bool drop_master;
};

static int check_config(struct drm_device *dev, struct drm_atomic_commit *state)
{
	struct setcrtc_fixture *f = dev->dev_private;
	int ret = drm_atomic_helper_check(dev, state);

	f->checks++;
	complete_all(&f->checked);
	return ret;
}

static int install_config(struct drm_device *dev, struct drm_atomic_commit *state,
			  bool nonblock)
{
	struct setcrtc_fixture *f = dev->dev_private;
	int ret = drm_atomic_helper_swap_state(state, false);

	if (!ret)
		f->installs++;
	return ret;
}

static const struct drm_mode_config_funcs config_funcs = {
	.atomic_check = check_config,
	.atomic_commit = install_config,
};

static const struct drm_crtc_funcs crtc_funcs = {
	.reset = drm_atomic_helper_crtc_reset,
	.atomic_duplicate_state = drm_atomic_helper_crtc_duplicate_state,
	.atomic_destroy_state = drm_atomic_helper_crtc_destroy_state,
	.set_config = drm_atomic_helper_set_config,
	.set_config_request = drm_atomic_helper_set_config_request,
};

static const struct file_operations file_ops = {
	.owner = THIS_MODULE,
	.release = drm_release_noglobal,
};

static const struct drm_driver driver = {
	.driver_features = DRIVER_MODESET | DRIVER_ATOMIC,
	.fops = &file_ops,
};

static void close_file(void *data)
{
	__fput_sync(data);
}

static void finish_fixture(void *data)
{
	struct setcrtc_fixture *f = data;

	complete_all(&f->checked);
	if (f->worker)
		kthread_stop(f->worker);
	if (f->read)
		drm_prepare_read_abandon(f->read);
	if (f->source)
		drm_prepare_source_put(f->source);
}

static struct setcrtc_fixture *new_setcrtc(struct kunit *test)
{
	struct setcrtc_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct device *parent = drm_kunit_helper_alloc_device(test);
	struct drm_master *master;
	struct drm_file *priv;
	struct drm_plane *plane;

	KUNIT_ASSERT_NOT_NULL(test, f);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	f->dev = __drm_kunit_helper_alloc_drm_device_with_driver(test, parent,
							      sizeof(*f->dev), 0, &driver);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->dev);
	f->dev->mode_config.funcs = &config_funcs;
	f->dev->dev_private = f;
	KUNIT_ASSERT_EQ(test, drm_atomic_prepare_display_init(f->dev, 8), 0);
	plane = drm_kunit_helper_create_primary_plane(test, f->dev, NULL, NULL, NULL, 0, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, plane);
	f->crtc = drm_kunit_helper_create_crtc(test, f->dev, plane, NULL, &crtc_funcs, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->crtc);
	drm_mode_config_reset(f->dev);
	f->file = mock_drm_getfile(f->dev->primary, O_RDWR);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->file);
	atomic_inc(&f->dev->open_count);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, close_file, f->file), 0);
	priv = f->file->private_data;
	master = kzalloc_obj(*master);
	KUNIT_ASSERT_NOT_NULL(test, master);
	kref_init(&master->refcount);
	master->dev = f->dev;
	idr_init_base(&master->magic_map, 1);
	idr_init(&master->leases);
	idr_init_base(&master->lessee_idr, 1);
	INIT_LIST_HEAD(&master->lessees);
	INIT_LIST_HEAD(&master->lessee_list);
	priv->master = master;
	priv->is_master = true;
	priv->was_master = true;
	mutex_lock(&f->dev->master_mutex);
	f->dev->master = drm_master_get(master);
	mutex_unlock(&f->dev->master_mutex);
	init_completion(&f->checked);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, finish_fixture, f), 0);
	return f;
}

static int finish_reader(void *data)
{
	struct setcrtc_fixture *f = data;
	struct drm_modeset_acquire_ctx ctx;

	wait_for_completion(&f->checked);
	drm_modeset_acquire_init(&ctx, 0);
	f->worker_error = drm_modeset_lock(&f->crtc->mutex, &ctx);
	drm_modeset_drop_locks(&ctx);
	drm_modeset_acquire_fini(&ctx);
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

static void start_reader(struct kunit *test, struct setcrtc_fixture *f)
{
	struct drm_prepare_source *source;
	struct drm_prepare_read_claim *read = NULL;
	struct task_struct *worker;

	KUNIT_ASSERT_EQ(test, drm_modeset_lock(&f->crtc->mutex, NULL), 0);
	source = drm_atomic_prepare_crtc_source(f->crtc);
	if (!IS_ERR(source)) {
		f->source = drm_prepare_source_get(source);
		read = drm_prepare_source_claim(source);
	}
	drm_modeset_unlock(&f->crtc->mutex);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, source);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, read);
	f->read = read;
	worker = kthread_run(finish_reader, f, "drm-setcrtc-reader");
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, worker);
	f->worker = worker;
}

static void join_reader(struct setcrtc_fixture *f)
{
	kthread_stop(f->worker);
	f->worker = NULL;
}

static void setcrtc_disable_waits_for_reader(struct kunit *test)
{
	struct setcrtc_fixture *f = new_setcrtc(test);
	struct drm_mode_crtc request = { .crtc_id = f->crtc->base.id };

	start_reader(test, f);
	KUNIT_EXPECT_EQ(test, drm_mode_setcrtc(f->dev, &request, f->file->private_data), 0);
	join_reader(f);
	KUNIT_EXPECT_EQ(test, f->worker_error, 0);
	KUNIT_EXPECT_EQ(test, f->checks, 2);
	KUNIT_EXPECT_EQ(test, f->installs, 1);
}

static void master_loss_cancels_setcrtc(struct kunit *test)
{
	struct setcrtc_fixture *f = new_setcrtc(test);
	struct drm_mode_crtc request = { .crtc_id = f->crtc->base.id };
	struct drm_crtc_state *before = f->crtc->state;

	f->drop_master = true;
	start_reader(test, f);
	KUNIT_EXPECT_EQ(test, drm_mode_setcrtc(f->dev, &request, f->file->private_data),
			-ECANCELED);
	join_reader(f);
	KUNIT_EXPECT_EQ(test, f->worker_error, 0);
	KUNIT_EXPECT_NOT_NULL(test, f->read);
	KUNIT_EXPECT_EQ(test, f->installs, 0);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state, before);
}

static struct kunit_case cases[] = {
	KUNIT_CASE(setcrtc_disable_waits_for_reader),
	KUNIT_CASE(master_loss_cancels_setcrtc),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_prepare_setcrtc",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_LICENSE("GPL");
MODULE_IMPORT_NS("EXPORTED_FOR_KUNIT_TESTING");
