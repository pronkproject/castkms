// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/file.h>
#include <linux/completion.h>
#include <linux/kthread.h>
#include <linux/mman.h>
#include <linux/module.h>
#include <linux/uaccess.h>
#include <drm/drm_auth.h>
#include <drm/drm_constraints.h>
#include <drm/drm_constraints_device.h>
#include <drm/drm_constraints_entry.h>
#include <drm/drm_constraints_list.h>
#include <drm/drm_constraints_output.h>
#include <drm/drm_constraints_owner.h>
#include <drm/drm_crtc.h>
#include <drm/drm_file.h>
#include <drm/drm_fourcc.h>
#include <drm/drm_kunit_helpers.h>
#include <uapi/drm/drm_constraints.h>
#include <kunit/test.h>

#include "../drm_constraints_events.h"
#include "../drm_internal.h"

static const struct file_operations test_fops = {
	.owner = THIS_MODULE,
	.release = drm_release_noglobal,
	.read = drm_read,
};

static const struct drm_driver test_driver = {
	.driver_features = DRIVER_MODESET | DRIVER_ATOMIC,
	.fops = &test_fops,
};

struct event_fixture {
	struct drm_device *dev;
	struct drm_crtc *crtc[2];
	struct file *file;
	struct drm_constraints_events *events;
	unsigned long address;
};

static void close_file(void *data) { __fput_sync(data); }
static void stop_owner(void *data) { drm_constraints_owner_stop(data); }
static void put_description(void *data) { drm_constraints_description_put(data); }
static void put_entry(void *data) { drm_constraints_entry_put(data); }
static void destroy_events(void *data) { drm_constraints_events_destroy(data); }
static void put_master(void *data)
{
	struct drm_master *master = data;

	drm_master_put(&master);
}

static int check(const struct drm_atomic_commit *state, const struct drm_crtc_state *crtc,
		 const struct drm_constraints_entry *entry)
{
	return 0;
}

static const struct drm_constraints_output_ops output_ops = { .check = check };

static struct event_fixture *new_fixture(struct kunit *test)
{
	struct event_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct device *parent = drm_kunit_helper_alloc_device(test);
	const struct drm_constraints_size size = { 640, 480, 640, 480 };
	struct drm_constraints_format format = {
		.format = DRM_FORMAT_XRGB8888, .size = size,
		.flags = DRM_CONSTRAINTS_FORMAT_IMPLICIT,
		.storage_flags = DRM_CONSTRAINTS_FORMAT_STORAGE_NATIVE,
		.pitch_alignment = 1, .offset_alignment = 1, .max_pitch = U32_MAX,
	};
	struct drm_constraints_description *description;
	struct drm_constraints_entry *entry;
	struct drm_plane *plane;
	unsigned int i;

	if (!IS_ENABLED(CONFIG_MMU))
		kunit_skip(test, "event copyout requires MMU");
	KUNIT_ASSERT_NOT_NULL(test, f);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	f->dev = __drm_kunit_helper_alloc_drm_device_with_driver(test, parent,
							      sizeof(*f->dev), 0, &test_driver);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->dev);
	KUNIT_ASSERT_EQ(test, drm_constraints_device_init(f->dev, 8), 0);
	for (i = 0; i < ARRAY_SIZE(f->crtc); i++) {
		plane = drm_kunit_helper_create_primary_plane(test, f->dev,
							    NULL, NULL, NULL, 0, NULL);
		KUNIT_ASSERT_NOT_ERR_OR_NULL(test, plane);
		format.plane_id = plane->base.id;
		f->crtc[i] = drm_kunit_helper_create_crtc(test, f->dev, plane, NULL, NULL, NULL);
		KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->crtc[i]);
		description = drm_constraints_description_create(&size, &format, 1, NULL, 0);
		KUNIT_ASSERT_NOT_ERR_OR_NULL(test, description);
		KUNIT_ASSERT_EQ(test,
				kunit_add_action_or_reset(test, put_description, description), 0);
		entry = drm_constraints_entry_create_stateless(
			drm_constraints_device_domain(f->dev), f->crtc[i]->base.id, description);
		KUNIT_ASSERT_NOT_ERR_OR_NULL(test, entry);
		KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_entry, entry), 0);
		KUNIT_ASSERT_EQ(test,
				drm_constraints_crtc_init(f->crtc[i], entry, 4, &output_ops), 0);
	}
	drm_mode_config_reset(f->dev);
	f->file = mock_drm_getfile(f->dev->primary, O_RDWR | O_NONBLOCK);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->file);
	atomic_inc(&f->dev->open_count);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, close_file, f->file), 0);
	KUNIT_ASSERT_EQ(test, drm_master_open(f->file->private_data), 0);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, stop_owner, f->dev), 0);
	f->events = drm_constraints_events_create(f->file->private_data);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->events);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, destroy_events, f->events), 0);
	drm_constraints_events_start(f->events);
	f->address = kunit_vm_mmap(test, NULL, 0, PAGE_SIZE, PROT_READ | PROT_WRITE,
				   MAP_PRIVATE | MAP_ANONYMOUS, 0);
	KUNIT_ASSERT_NE(test, f->address, 0);
	KUNIT_ASSERT_FALSE(test, IS_ERR_VALUE(f->address));
	drm_constraints_events_flush(f->events);
	return f;
}

static u64 suggest(struct kunit *test, struct event_fixture *f, unsigned int index, bool selected)
{
	struct drm_constraints_list *list = drm_constraints_crtc_list(f->crtc[index]);
	u64 id = drm_constraints_entry_id(f->crtc[index]->state->constraints);
	u64 generation;

	KUNIT_ASSERT_EQ(test, drm_constraints_list_suggest(list, selected ? id : 0), 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_list_observe(list, &generation), 0);
	drm_constraints_events_flush(f->events);
	return generation;
}

static void read_event(struct kunit *test, struct event_fixture *f, unsigned int index,
		       u64 generation, u32 flags)
{
	struct drm_event_kms_constraints_list_changed event;
	loff_t offset = 0;

	KUNIT_ASSERT_EQ(test, drm_read(f->file, (char __user *)f->address, sizeof(event), &offset),
			sizeof(event));
	KUNIT_ASSERT_EQ(test, copy_from_user(&event, (void __user *)f->address, sizeof(event)), 0);
	KUNIT_EXPECT_EQ(test, event.base.type, DRM_EVENT_KMS_CONSTRAINTS_LIST_CHANGED);
	KUNIT_EXPECT_EQ(test, event.base.length, sizeof(event));
	KUNIT_EXPECT_EQ(test, event.crtc_id, f->crtc[index]->base.id);
	KUNIT_EXPECT_EQ(test, event.generation, generation);
	KUNIT_EXPECT_EQ(test, event.flags, flags);
	KUNIT_EXPECT_EQ(test, event.reserved, 0);
}

static void expect_empty(struct kunit *test, struct event_fixture *f)
{
	loff_t offset = 0;

	drm_constraints_events_flush(f->events);
	KUNIT_EXPECT_EQ(test,
			drm_read(f->file, (char __user *)f->address, PAGE_SIZE, &offset), -EAGAIN);
}

static void changes_coalesce_behind_one_immutable_event(struct kunit *test)
{
	struct event_fixture *f = new_fixture(test);
	u64 first, latest = 0;
	unsigned int i;

	expect_empty(test, f);
	first = suggest(test, f, 0, true);
	for (i = 0; i < 100; i++)
		latest = suggest(test, f, 0, i & 1);
	read_event(test, f, 0, first, 0);
	drm_constraints_events_flush(f->events);
	read_event(test, f, 0, latest, 0);
	expect_empty(test, f);
}

static void failed_reads_preserve_the_queued_generation(struct kunit *test)
{
	struct event_fixture *f = new_fixture(test);
	u64 first = suggest(test, f, 0, true);
	u64 latest = suggest(test, f, 0, false);
	loff_t offset = 0;

	KUNIT_EXPECT_EQ(test, drm_read(f->file, (char __user *)f->address, 1, &offset), 0);
	KUNIT_EXPECT_EQ(test, drm_read(f->file, (char __user *)1, PAGE_SIZE, &offset), -EFAULT);
	read_event(test, f, 0, first, 0);
	drm_constraints_events_flush(f->events);
	read_event(test, f, 0, latest, 0);
	expect_empty(test, f);
}

struct full_queue {
	struct drm_device *dev;
	struct drm_pending_event *pending;
};

static void cancel_full_queue(void *data)
{
	struct full_queue *full = data;

	if (full->pending) {
		drm_event_cancel_free(full->dev, full->pending);
		full->pending = NULL;
	}
}

static struct full_queue *reserve_capacity(struct kunit *test, struct event_fixture *f,
					  u32 remaining)
{
	struct drm_file *priv = f->file->private_data;
	struct full_queue *full = kunit_kzalloc(test, sizeof(*full), GFP_KERNEL);
	struct drm_pending_event *pending;
	struct drm_event *event;
	int ret;

	KUNIT_ASSERT_NOT_NULL(test, full);
	KUNIT_ASSERT_GE(test, priv->event_space, remaining + sizeof(*event));
	pending = kzalloc(sizeof(*pending) + priv->event_space, GFP_KERNEL);
	KUNIT_ASSERT_NOT_NULL(test, pending);
	event = (void *)(pending + 1);
	event->type = DRM_EVENT_VBLANK;
	event->length = priv->event_space - remaining;
	ret = drm_event_reserve_init(f->dev, priv, pending, event);
	if (ret)
		kfree(pending);
	KUNIT_ASSERT_EQ(test, ret, 0);
	full->dev = f->dev;
	full->pending = pending;
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, cancel_full_queue, full), 0);
	return full;
}

static void returned_queue_space_retries_pending_changes(struct kunit *test)
{
	struct event_fixture *f = new_fixture(test);
	struct full_queue *full = reserve_capacity(test, f, 0);
	u64 generation = suggest(test, f, 0, true);

	expect_empty(test, f);
	cancel_full_queue(full);
	drm_constraints_events_flush(f->events);
	read_event(test, f, 0, generation, 0);
	expect_empty(test, f);
}

static void constrained_queue_capacity_does_not_starve_other_outputs(struct kunit *test)
{
	struct event_fixture *f = new_fixture(test);
	struct full_queue *full =
		reserve_capacity(test, f, sizeof(struct drm_event_kms_constraints_list_changed));
	u64 first = suggest(test, f, 0, true);
	u64 other = suggest(test, f, 1, true);
	u64 latest = suggest(test, f, 0, false);

	read_event(test, f, 0, first, 0);
	drm_constraints_events_flush(f->events);
	read_event(test, f, 1, other, 0);
	drm_constraints_events_flush(f->events);
	read_event(test, f, 0, latest, 0);
	cancel_full_queue(full);
	expect_empty(test, f);
}

static void output_notifications_are_independent(struct kunit *test)
{
	struct event_fixture *f = new_fixture(test);
	u64 first = suggest(test, f, 0, true);
	u64 second = suggest(test, f, 1, true);

	read_event(test, f, 0, first, 0);
	read_event(test, f, 1, second, 0);
	expect_empty(test, f);
}

static void closure_is_delivered_after_an_outstanding_generation(struct kunit *test)
{
	struct event_fixture *f = new_fixture(test);
	u64 first = suggest(test, f, 0, true);

	drm_constraints_list_close(drm_constraints_crtc_list(f->crtc[0]));
	drm_constraints_events_flush(f->events);
	read_event(test, f, 0, first, 0);
	drm_constraints_events_flush(f->events);
	read_event(test, f, 0, 0, DRM_KMS_CONSTRAINTS_LIST_CLOSED);
	expect_empty(test, f);
}

static void queued_storage_outlives_observer_destruction(struct kunit *test)
{
	struct event_fixture *f = new_fixture(test);
	u64 first = suggest(test, f, 0, true);
	loff_t offset = 0;

	kunit_release_action(test, destroy_events, f->events);
	f->events = NULL;
	KUNIT_ASSERT_EQ(test,
			drm_constraints_list_suggest(drm_constraints_crtc_list(f->crtc[0]), 0), 0);
	read_event(test, f, 0, first, 0);
	KUNIT_EXPECT_EQ(test,
			drm_read(f->file, (char __user *)f->address, PAGE_SIZE, &offset), -EAGAIN);
}

static void master_loss_stops_new_notifications(struct kunit *test)
{
	struct event_fixture *f = new_fixture(test);

	kunit_release_action(test, stop_owner, f->dev);
	KUNIT_ASSERT_EQ(test, drm_dropmaster_ioctl(f->dev, NULL, f->file->private_data), 0);
	suggest(test, f, 0, true);
	expect_empty(test, f);
}

static void queued_slots_are_reclaimed_during_file_close(struct kunit *test)
{
	struct event_fixture *f = new_fixture(test);

	suggest(test, f, 0, true);
	suggest(test, f, 1, true);
	kunit_release_action(test, destroy_events, f->events);
	f->events = NULL;
	kunit_release_action(test, stop_owner, f->dev);
	kunit_release_action(test, close_file, f->file);
	f->file = NULL;
}

static void lease_revocation_excludes_new_events(struct kunit *test)
{
	struct event_fixture *f = new_fixture(test);
	struct drm_master *root = ((struct drm_file *)f->file->private_data)->master;
	struct drm_master *lease = drm_master_create(f->dev);
	struct drm_file *priv;
	u64 generation;
	int ret;

	KUNIT_ASSERT_NOT_NULL(test, lease);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_master, lease), 0);
	mutex_lock(&f->dev->mode_config.idr_mutex);
	lease->lessor = drm_master_get(root);
	list_add_tail(&lease->lessee_list, &root->lessees);
	mutex_unlock(&f->dev->mode_config.idr_mutex);
	kunit_release_action(test, destroy_events, f->events);
	f->file = mock_drm_getfile(f->dev->primary, O_RDWR | O_NONBLOCK);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->file);
	atomic_inc(&f->dev->open_count);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, close_file, f->file), 0);
	priv = f->file->private_data;
	if (priv->master)
		drm_master_put(&priv->master);
	priv->master = drm_master_get(lease);
	priv->is_master = true;
	f->events = drm_constraints_events_create(priv);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->events);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, destroy_events, f->events), 0);
	drm_constraints_events_start(f->events);
	suggest(test, f, 0, true);
	expect_empty(test, f);
	mutex_lock(&f->dev->mode_config.idr_mutex);
	ret = idr_alloc(&lease->leases, f->crtc[0], f->crtc[0]->base.id,
			f->crtc[0]->base.id + 1, GFP_KERNEL);
	mutex_unlock(&f->dev->mode_config.idr_mutex);
	KUNIT_ASSERT_EQ(test, ret, f->crtc[0]->base.id);
	generation = suggest(test, f, 0, false);
	suggest(test, f, 1, true);
	mutex_lock(&f->dev->mode_config.idr_mutex);
	idr_remove(&lease->leases, f->crtc[0]->base.id);
	mutex_unlock(&f->dev->mode_config.idr_mutex);
	suggest(test, f, 0, true);
	/* Revocation does not rewrite or discard previously queued history. */
	read_event(test, f, 0, generation, 0);
	expect_empty(test, f);
}

static void event_encoding_has_fixed_compat_layout(struct kunit *test)
{
	KUNIT_EXPECT_EQ(test, sizeof(struct drm_event_kms_constraints_list_changed), 32);
	KUNIT_EXPECT_EQ(test, offsetof(struct drm_event_kms_constraints_list_changed, crtc_id), 8);
	KUNIT_EXPECT_EQ(test,
			offsetof(struct drm_event_kms_constraints_list_changed, generation), 16);
	KUNIT_EXPECT_EQ(test,
			offsetof(struct drm_event_kms_constraints_list_changed, reserved), 24);
}

struct publishing_thread {
	struct drm_constraints_list *list;
	u64 id;
	struct completion started;
};

static int publish_changes(void *data)
{
	struct publishing_thread *publisher = data;
	unsigned int i;
	int ret;

	for (i = 0; i < 1024; i++) {
		ret = drm_constraints_list_suggest(publisher->list, i & 1 ? 0 : publisher->id);
		if (!i)
			complete(&publisher->started);
		if (ret)
			break;
		if (kthread_should_stop())
			break;
		cond_resched();
	}
	/* Keep the task alive until its owner joins it, including early errors. */
	set_current_state(TASK_INTERRUPTIBLE);
	while (!kthread_should_stop()) {
		schedule();
		set_current_state(TASK_INTERRUPTIBLE);
	}
	__set_current_state(TASK_RUNNING);
	return ret;
}

static void stop_thread(void *data) { kthread_stop(data); }

static void publication_can_race_observer_destruction(struct kunit *test)
{
	struct event_fixture *f = new_fixture(test);
	struct publishing_thread *publisher = kunit_kzalloc(test, sizeof(*publisher), GFP_KERNEL);
	struct task_struct *task;

	KUNIT_ASSERT_NOT_NULL(test, publisher);
	publisher->list = drm_constraints_crtc_list(f->crtc[0]);
	publisher->id = drm_constraints_entry_id(f->crtc[0]->state->constraints);
	init_completion(&publisher->started);
	task = kthread_run(publish_changes, publisher, "drm-constraints-publish");
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, task);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, stop_thread, task), 0);
	KUNIT_ASSERT_NE(test, wait_for_completion_timeout(&publisher->started, HZ), 0);
	kunit_release_action(test, destroy_events, f->events);
	f->events = NULL;
	KUNIT_EXPECT_EQ(test, kthread_stop(task), 0);
	kunit_remove_action(test, stop_thread, task);
}

static struct kunit_case event_cases[] = {
	KUNIT_CASE(changes_coalesce_behind_one_immutable_event),
	KUNIT_CASE(failed_reads_preserve_the_queued_generation),
	KUNIT_CASE(returned_queue_space_retries_pending_changes),
	KUNIT_CASE(constrained_queue_capacity_does_not_starve_other_outputs),
	KUNIT_CASE(output_notifications_are_independent),
	KUNIT_CASE(closure_is_delivered_after_an_outstanding_generation),
	KUNIT_CASE(queued_storage_outlives_observer_destruction),
	KUNIT_CASE(master_loss_stops_new_notifications),
	KUNIT_CASE(queued_slots_are_reclaimed_during_file_close),
	KUNIT_CASE(lease_revocation_excludes_new_events),
	KUNIT_CASE(event_encoding_has_fixed_compat_layout),
	KUNIT_CASE(publication_can_race_observer_destruction),
	{}
};

static struct kunit_suite event_suite = {
	.name = "drm_constraints_events",
	.test_cases = event_cases,
};
kunit_test_suite(event_suite);

MODULE_LICENSE("GPL and additional rights");
