// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/completion.h>
#include <linux/file.h>
#include <linux/kthread.h>
#include <drm/drm_atomic.h>
#include <drm/drm_atomic_helper.h>
#include <drm/drm_atomic_power.h>
#include <drm/drm_atomic_prepare.h>
#include <drm/drm_atomic_prepare_auth.h>
#include <drm/drm_atomic_prepare_display.h>
#include <drm/drm_atomic_prepare_owner.h>
#include <drm/drm_atomic_uapi.h>
#include <drm/drm_auth.h>
#include <drm/drm_connector.h>
#include <drm/drm_file.h>
#include <drm/drm_ioctl.h>
#include <drm/drm_kunit_helpers.h>
#include <kunit/test.h>

#include "../drm_crtc_internal.h"

struct power_fixture {
	struct drm_device *dev;
	struct drm_crtc *crtc;
	struct drm_connector connector;
	struct file *file;
	struct drm_prepare_owner *owner;
	struct drm_prepare_read_claim *read;
	struct task_struct *worker;
	struct completion checked;
	unsigned int checks, installs;
	int worker_error, power_while_waiting;
};

static int check_update(struct drm_device *dev, struct drm_atomic_commit *state)
{
	struct power_fixture *f = dev->dev_private;
	struct drm_crtc *crtc;
	struct drm_crtc_state *old, *new;
	int i;

	/* The fixture models power changes without encoders or hardware checks. */
	for_each_oldnew_crtc_in_state(state, crtc, old, new, i)
		new->active_changed = old->active != new->active;
	f->checks++;
	complete_all(&f->checked);
	return 0;
}

static int install_update(struct drm_device *dev, struct drm_atomic_commit *state, bool nonblock)
{
	struct power_fixture *f = dev->dev_private;
	int ret = drm_atomic_helper_swap_state(state, false);

	if (!ret)
		f->installs++;
	return ret;
}

static const struct drm_mode_config_funcs config_funcs = {
	.atomic_check = check_update,
	.atomic_commit = install_update,
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

static void close_file(void *data)
{
	__fput_sync(data);
}

static void finish_fixture(void *data)
{
	struct power_fixture *f = data;

	complete_all(&f->checked);
	if (f->worker)
		kthread_stop(f->worker);
	if (f->read)
		drm_prepare_read_abandon(f->read);
	drm_prepare_owner_put(f->owner);
}

static struct power_fixture *new_fixture(struct kunit *test)
{
	const struct drm_display_mode mode = {
		DRM_MODE("64x64", 0, 1000, 64, 65, 66, 67, 0, 64, 65, 66, 67, 0, 0)
	};
	struct power_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct device *parent = drm_kunit_helper_alloc_device(test);
	struct drm_plane *primary;
	struct drm_file *file;
	struct drm_master *master;
	void *old;
	int ret;

	KUNIT_ASSERT_NOT_NULL(test, f);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	f->dev = __drm_kunit_helper_alloc_drm_device_with_driver(test, parent,
							   sizeof(*f->dev), 0, &driver);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->dev);
	f->dev->dev_private = f;
	f->dev->mode_config.funcs = &config_funcs;
	KUNIT_ASSERT_EQ(test, drm_atomic_prepare_display_init(f->dev, 8), 0);
	primary = drm_kunit_helper_create_primary_plane(test, f->dev, NULL, NULL, NULL, 0, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, primary);
	f->crtc = drm_kunit_helper_create_crtc(test, f->dev, primary, NULL, NULL, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->crtc);
	KUNIT_ASSERT_EQ(test, drmm_connector_init(f->dev, &f->connector, &connector_funcs,
						DRM_MODE_CONNECTOR_VIRTUAL, NULL), 0);
	/* Publish the connector ID without creating sysfs or debugfs entries. */
	mutex_lock(&f->dev->mode_config.idr_mutex);
	old = idr_replace(&f->dev->mode_config.object_idr, &f->connector.base,
			  f->connector.base.id);
	mutex_unlock(&f->dev->mode_config.idr_mutex);
	KUNIT_ASSERT_NULL(test, old);
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
	f->owner = drm_file_prepare_owner(file);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->owner);
	init_completion(&f->checked);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, finish_fixture, f), 0);
	ret = drm_modeset_lock(&f->crtc->mutex, NULL);
	KUNIT_ASSERT_EQ(test, ret, 0);
	ret = drm_atomic_set_mode_for_crtc(f->crtc->state, &mode);
	f->crtc->state->active = true;
	f->crtc->state->connector_mask = drm_connector_mask(&f->connector);
	drm_connector_get(&f->connector);
	f->connector.state->crtc = f->crtc;
	f->connector.dpms = DRM_MODE_DPMS_ON;
	drm_modeset_unlock(&f->crtc->mutex);
	KUNIT_ASSERT_EQ(test, ret, 0);
	return f;
}

static int finish_reader(void *data)
{
	struct power_fixture *f = data;
	struct drm_modeset_acquire_ctx ctx;
	int ret;

	wait_for_completion(&f->checked);
	drm_modeset_acquire_init(&ctx, 0);
	for (;;) {
		ret = drm_modeset_lock_all_ctx(f->dev, &ctx);
		if (ret != -EDEADLK)
			break;
		ret = drm_modeset_backoff(&ctx);
		if (ret)
			break;
	}
	if (!ret)
		f->power_while_waiting = f->connector.dpms;
	drm_modeset_drop_locks(&ctx);
	drm_modeset_acquire_fini(&ctx);
	f->worker_error = ret;
	drm_prepare_read_release(f->read, NULL);
	f->read = NULL;
	set_current_state(TASK_INTERRUPTIBLE);
	while (!kthread_should_stop()) {
		schedule();
		set_current_state(TASK_INTERRUPTIBLE);
	}
	__set_current_state(TASK_RUNNING);
	return 0;
}

static void start_reader(struct kunit *test, struct power_fixture *f)
{
	struct drm_prepare_source *source;
	struct drm_prepare_read_claim *read = NULL;
	int ret = drm_modeset_lock(&f->crtc->mutex, NULL);

	KUNIT_ASSERT_EQ(test, ret, 0);
	source = drm_atomic_prepare_crtc_source(f->crtc);
	if (!IS_ERR(source))
		read = drm_prepare_source_claim(source);
	drm_modeset_unlock(&f->crtc->mutex);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, source);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, read);
	f->read = read;
	f->worker = kthread_run(finish_reader, f, "drm-power-reader");
	if (IS_ERR(f->worker)) {
		ret = PTR_ERR(f->worker);
		f->worker = NULL;
	}
	KUNIT_ASSERT_EQ(test, ret, 0);
}

static void join_reader(struct kunit *test, struct power_fixture *f)
{
	complete_all(&f->checked);
	kthread_stop(f->worker);
	f->worker = NULL;
	KUNIT_EXPECT_EQ(test, f->worker_error, 0);
}

static int set_power(struct power_fixture *f, u64 mode)
{
	struct drm_mode_obj_set_property args = {
		.obj_id = f->connector.base.id, .obj_type = DRM_MODE_OBJECT_CONNECTOR,
		.prop_id = f->dev->mode_config.dpms_property->base.id, .value = mode,
	};

	return drm_mode_obj_set_property_ioctl(f->dev, &args, f->file->private_data);
}

static void power_ioctl_waits_without_publishing_preference(struct kunit *test)
{
	struct power_fixture *f = new_fixture(test);
	int ret;

	start_reader(test, f);
	ret = set_power(f, DRM_MODE_DPMS_OFF);
	join_reader(test, f);
	KUNIT_EXPECT_EQ(test, ret, 0);
	KUNIT_EXPECT_GE(test, f->checks, 2);
	KUNIT_EXPECT_EQ(test, f->power_while_waiting, DRM_MODE_DPMS_ON);
	KUNIT_EXPECT_EQ(test, f->installs, 1);
	KUNIT_EXPECT_EQ(test, f->connector.dpms, DRM_MODE_DPMS_OFF);
	KUNIT_EXPECT_FALSE(test, f->crtc->state->active);
}

static struct kunit_case cases[] = {
	KUNIT_CASE(power_ioctl_waits_without_publishing_preference),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_prepare_power",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_LICENSE("GPL");
MODULE_IMPORT_NS("EXPORTED_FOR_KUNIT_TESTING");
