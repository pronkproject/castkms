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
#include <drm/drm_auth.h>
#include <drm/drm_file.h>
#include <drm/drm_ioctl.h>
#include <drm/drm_kunit_helpers.h>
#include <drm/drm_property.h>
#include <drm/drm_vblank.h>
#include <kunit/test.h>

#include "../drm_atomic_user_commit.h"
#include "../drm_atomic_user_input.h"

struct commit_fixture {
	struct drm_device *dev;
	struct drm_crtc *crtc;
	struct file *file;
	struct drm_prepare_owner *owner;
	struct drm_prepare_source *source;
	struct drm_prepare_read_claim *read;
	struct task_struct *worker;
	struct completion checked;
	struct drm_atomic_user_input input;
	u32 object, count, property;
	u64 value;
	unsigned int checks, installations;
	int worker_error;
	bool lose_master, revoke, abandon;
	struct drm_crtc *added_crtc;
	unsigned int events;
};

static int check_request(struct drm_device *dev, struct drm_atomic_commit *state)
{
	struct commit_fixture *f = dev->dev_private;
	int ret = drm_atomic_helper_check(dev, state);

	if (!ret && f->added_crtc) {
		struct drm_crtc_state *added = drm_atomic_get_crtc_state(state, f->added_crtc);

		if (IS_ERR(added))
			ret = PTR_ERR(added);
	}
	f->checks++;
	complete_all(&f->checked);
	return ret;
}

static int install_request(struct drm_device *dev, struct drm_atomic_commit *state, bool nonblock)
{
	struct commit_fixture *f = dev->dev_private;
	int ret = drm_atomic_helper_swap_state(state, false);

	if (!ret) {
		struct drm_crtc *crtc;
		struct drm_crtc_state *new;
		int i;

		f->installations++;
		for_each_new_crtc_in_state(state, crtc, new, i) {
			if (!new->event)
				continue;
			f->events++;
			drm_event_cancel_free(dev, &new->event->base);
			new->event = NULL;
		}
	}
	return ret;
}

static const struct drm_mode_config_funcs config_funcs = {
	.atomic_check = check_request,
	.atomic_commit = install_request,
};

static const struct file_operations test_fops = {
	.owner = THIS_MODULE,
	.release = drm_release_noglobal,
};

static const struct drm_driver test_driver = {
	.driver_features = DRIVER_MODESET | DRIVER_ATOMIC,
	.fops = &test_fops,
};

static void close_file(void *data)
{
	__fput_sync(data);
}

static void finish_fixture(void *data)
{
	struct commit_fixture *f = data;
	struct drm_file *file = f->file->private_data;

	complete_all(&f->checked);
	if (f->worker)
		kthread_stop(f->worker);
	if (f->read)
		drm_prepare_read_abandon(f->read);
	if (f->source)
		drm_prepare_source_put(f->source);
	drm_prepare_owner_put(f->owner);
	mutex_lock(&f->dev->master_mutex);
	file->is_master = true;
	mutex_unlock(&f->dev->master_mutex);
}

static struct commit_fixture *new_fixture(struct kunit *test)
{
	struct commit_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct device *parent = drm_kunit_helper_alloc_device(test);
	struct drm_plane *plane;
	struct drm_file *file;
	struct drm_master *master;

	KUNIT_ASSERT_NOT_NULL(test, f);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	f->dev = __drm_kunit_helper_alloc_drm_device_with_driver(test, parent,
							   sizeof(*f->dev), 0, &test_driver);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->dev);
	f->dev->dev_private = f;
	f->dev->mode_config.funcs = &config_funcs;
	KUNIT_ASSERT_EQ(test, drm_atomic_prepare_display_init(f->dev, 8), 0);
	plane = drm_kunit_helper_create_primary_plane(test, f->dev, NULL, NULL, NULL, 0, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, plane);
	f->crtc = drm_kunit_helper_create_crtc(test, f->dev, plane, NULL, NULL, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->crtc);
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
	file->atomic = true;
	mutex_lock(&f->dev->master_mutex);
	f->dev->master = drm_master_get(master);
	mutex_unlock(&f->dev->master_mutex);
	f->owner = drm_file_prepare_owner(file);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->owner);
	init_completion(&f->checked);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, finish_fixture, f), 0);
	f->object = f->crtc->base.id;
	f->count = 1;
	f->property = f->dev->mode_config.prop_active->base.id;
	f->input = (struct drm_atomic_user_input) {
		.object_count = 1, .property_count = 1,
		.objects = &f->object, .counts = &f->count,
		.properties = &f->property, .values = &f->value,
	};
	return f;
}

static int finish_reader(void *data)
{
	struct commit_fixture *f = data;
	struct drm_file *file = f->file->private_data;

	wait_for_completion(&f->checked);
	f->worker_error = drm_modeset_lock(&f->crtc->mutex, NULL);
	if (!f->worker_error) {
		WRITE_ONCE(f->value, 1);
		drm_modeset_unlock(&f->crtc->mutex);
		if (f->lose_master) {
			mutex_lock(&f->dev->master_mutex);
			file->is_master = false;
			mutex_unlock(&f->dev->master_mutex);
		}
		if (f->revoke)
			drm_prepare_owner_revoke(f->owner);
	}
	if (f->abandon)
		drm_prepare_read_abandon(f->read);
	else
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

static void start_reader(struct kunit *test, struct commit_fixture *f)
{
	struct drm_prepare_source *source;
	struct task_struct *worker;
	int ret = drm_modeset_lock(&f->crtc->mutex, NULL);

	KUNIT_ASSERT_EQ(test, ret, 0);
	source = drm_atomic_prepare_crtc_source(f->crtc);
	if (!IS_ERR(source)) {
		f->source = drm_prepare_source_get(source);
		f->read = drm_prepare_source_claim(source);
	}
	drm_modeset_unlock(&f->crtc->mutex);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, source);
	if (IS_ERR(f->read)) {
		ret = PTR_ERR(f->read);
		f->read = NULL;
		KUNIT_FAIL(test, "Cannot claim source: %d", ret);
		return;
	}
	worker = kthread_run(finish_reader, f, "drm-user-reader");
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, worker);
	f->worker = worker;
}

static int commit_request(struct commit_fixture *f, u32 flags)
{
	return drm_atomic_commit_user_request(f->dev, f->file->private_data,
					      f->owner, flags, 0, &f->input);
}

static void ready_request_installs_once(struct kunit *test)
{
	struct commit_fixture *f = new_fixture(test);

	KUNIT_EXPECT_EQ(test, commit_request(f, 0), 0);
	KUNIT_EXPECT_EQ(test, f->checks, 1);
	KUNIT_EXPECT_EQ(test, f->installations, 1);
}

static void waiting_request_does_not_reread_input(struct kunit *test)
{
	struct commit_fixture *f = new_fixture(test);

	start_reader(test, f);
	KUNIT_EXPECT_EQ(test, commit_request(f, 0), 0);
	KUNIT_EXPECT_EQ(test, f->worker_error, 0);
	KUNIT_EXPECT_GE(test, f->checks, 2);
	KUNIT_EXPECT_EQ(test, f->value, 1);
	KUNIT_EXPECT_FALSE(test, f->crtc->state->active);
	KUNIT_EXPECT_EQ(test, f->installations, 1);
}

static void master_loss_is_rechecked_after_wait(struct kunit *test)
{
	struct commit_fixture *f = new_fixture(test);

	f->lose_master = true;
	start_reader(test, f);
	KUNIT_EXPECT_EQ(test, commit_request(f, 0), -EACCES);
	KUNIT_EXPECT_EQ(test, f->worker_error, 0);
	KUNIT_EXPECT_EQ(test, f->installations, 0);
}

static void issuer_revocation_prevents_acceptance(struct kunit *test)
{
	struct commit_fixture *f = new_fixture(test);

	f->revoke = true;
	start_reader(test, f);
	KUNIT_EXPECT_EQ(test, commit_request(f, 0), -ECANCELED);
	KUNIT_EXPECT_EQ(test, f->worker_error, 0);
	KUNIT_EXPECT_EQ(test, f->installations, 0);
}

static void abandoned_source_prevents_acceptance(struct kunit *test)
{
	struct commit_fixture *f = new_fixture(test);

	f->abandon = true;
	start_reader(test, f);
	KUNIT_EXPECT_EQ(test, commit_request(f, 0), -EIO);
	KUNIT_EXPECT_EQ(test, f->worker_error, 0);
	KUNIT_EXPECT_EQ(test, f->installations, 0);
}

static struct commit_fixture *new_event_fixture(struct kunit *test)
{
	struct commit_fixture *f = new_fixture(test);
	struct drm_plane *plane;
	int ret;

	plane = drm_kunit_helper_create_primary_plane(test, f->dev, NULL, NULL, NULL, 0, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, plane);
	f->added_crtc = drm_kunit_helper_create_crtc(test, f->dev, plane, NULL, NULL, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->added_crtc);
	drm_mode_config_reset(f->dev);
	ret = drm_modeset_lock(&f->crtc->mutex, NULL);
	KUNIT_ASSERT_EQ(test, ret, 0);
	/* The requested disable may report completion for its previously active output. */
	f->crtc->state->active = true;
	drm_modeset_unlock(&f->crtc->mutex);
	return f;
}

static void driver_added_controller_does_not_request_an_event(struct kunit *test)
{
	struct commit_fixture *f = new_event_fixture(test);

	KUNIT_EXPECT_EQ(test, commit_request(f, DRM_MODE_PAGE_FLIP_EVENT |
					      DRM_MODE_ATOMIC_ALLOW_MODESET), 0);
	KUNIT_EXPECT_EQ(test, f->installations, 1);
	KUNIT_EXPECT_EQ(test, f->events, 1);
}

static void waiting_request_preserves_event_recipients(struct kunit *test)
{
	struct commit_fixture *f = new_event_fixture(test);

	start_reader(test, f);
	KUNIT_EXPECT_EQ(test, commit_request(f, DRM_MODE_PAGE_FLIP_EVENT |
					      DRM_MODE_ATOMIC_ALLOW_MODESET), 0);
	KUNIT_EXPECT_EQ(test, f->worker_error, 0);
	KUNIT_EXPECT_GE(test, f->checks, 2);
	KUNIT_EXPECT_EQ(test, f->installations, 1);
	KUNIT_EXPECT_EQ(test, f->events, 1);
}

static struct kunit_case cases[] = {
	KUNIT_CASE(driver_added_controller_does_not_request_an_event),
	KUNIT_CASE(waiting_request_preserves_event_recipients),
	KUNIT_CASE(ready_request_installs_once),
	KUNIT_CASE(waiting_request_does_not_reread_input),
	KUNIT_CASE(master_loss_is_rechecked_after_wait),
	KUNIT_CASE(issuer_revocation_prevents_acceptance),
	KUNIT_CASE(abandoned_source_prevents_acceptance),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_user_commit",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_IMPORT_NS("EXPORTED_FOR_KUNIT_TESTING");
MODULE_LICENSE("GPL");
