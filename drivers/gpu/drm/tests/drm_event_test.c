// SPDX-License-Identifier: GPL-2.0 OR MIT

/* DRM event queue readiness and ownership. */

#include <linux/file.h>
#include <linux/mman.h>
#include <linux/module.h>
#include <linux/slab.h>
#include <drm/drm_drv.h>
#include <drm/drm_file.h>
#include <drm/drm_kunit_helpers.h>
#include <uapi/drm/drm.h>
#include <kunit/test.h>

static const struct file_operations test_fops = {
	.owner = THIS_MODULE,
	.release = drm_release_noglobal,
	.read = drm_read,
};

static const struct drm_driver test_driver = {
	.driver_features = DRIVER_MODESET | DRIVER_ATOMIC,
	.fops = &test_fops,
};

struct space_fixture {
	struct drm_device *dev;
	struct file *file;
	wait_queue_entry_t wait;
	atomic_t wakes;
};

struct test_event {
	struct drm_pending_event pending;
	struct drm_event payload;
};

struct event_owner {
	struct space_fixture *fixture;
	struct test_event *event;
};

static int observe_space(wait_queue_entry_t *wait, unsigned int mode, int flags, void *key)
{
	struct space_fixture *f = container_of(wait, struct space_fixture, wait);

	atomic_inc(&f->wakes);
	return 0;
}

static void close_file(void *data) { __fput_sync(data); }

static void detach_waiter(void *data)
{
	struct space_fixture *f = data;
	struct drm_file *priv = f->file->private_data;

	remove_wait_queue(&priv->event_space_wait, &f->wait);
}

static struct space_fixture *new_fixture(struct kunit *test)
{
	struct space_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct device *parent = drm_kunit_helper_alloc_device(test);
	struct drm_file *priv;

	KUNIT_ASSERT_NOT_NULL(test, f);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	f->dev = __drm_kunit_helper_alloc_drm_device_with_driver(test, parent,
							      sizeof(*f->dev), 0, &test_driver);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->dev);
	f->file = mock_drm_getfile(f->dev->primary, O_RDWR | O_NONBLOCK);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->file);
	atomic_inc(&f->dev->open_count);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, close_file, f->file), 0);
	priv = f->file->private_data;
	atomic_set(&f->wakes, 0);
	init_waitqueue_func_entry(&f->wait, observe_space);
	add_wait_queue(&priv->event_space_wait, &f->wait);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, detach_waiter, f), 0);
	return f;
}

static void cancel_event(void *data)
{
	struct event_owner *owner = data;

	if (owner->event) {
		drm_event_cancel_free(owner->fixture->dev, &owner->event->pending);
		owner->event = NULL;
	}
}

static struct event_owner *new_event(struct kunit *test, struct space_fixture *f, u32 length)
{
	struct event_owner *owner = kunit_kzalloc(test, sizeof(*owner), GFP_KERNEL);
	struct drm_file *priv = f->file->private_data;
	struct test_event *event;
	int ret;

	KUNIT_ASSERT_NOT_NULL(test, owner);
	KUNIT_ASSERT_GE(test, length, (u32)sizeof(event->payload));
	event = kzalloc(sizeof(*event) + length - sizeof(event->payload), GFP_KERNEL);
	KUNIT_ASSERT_NOT_NULL(test, event);
	event->payload.type = DRM_EVENT_VBLANK;
	event->payload.length = length;
	ret = drm_event_reserve_init(f->dev, priv, &event->pending, &event->payload);
	if (ret)
		kfree(event);
	KUNIT_ASSERT_EQ(test, ret, 0);
	owner->fixture = f;
	owner->event = event;
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, cancel_event, owner), 0);
	return owner;
}

static void publish_event(struct event_owner *owner)
{
	drm_send_event(owner->fixture->dev, &owner->event->pending);
	owner->event = NULL;
}

static unsigned long user_page(struct kunit *test)
{
	unsigned long address;

	if (!IS_ENABLED(CONFIG_MMU))
		kunit_skip(test, "event reads require MMU");
	address = kunit_vm_mmap(test, NULL, 0, PAGE_SIZE, PROT_READ | PROT_WRITE,
			       MAP_PRIVATE | MAP_ANONYMOUS, 0);
	KUNIT_ASSERT_NE(test, address, 0);
	KUNIT_ASSERT_FALSE(test, IS_ERR_VALUE(address));
	return address;
}

static void cancellation_notifies_returned_capacity(struct kunit *test)
{
	struct space_fixture *f = new_fixture(test);
	struct drm_file *priv = f->file->private_data;
	int capacity = priv->event_space;
	struct event_owner *owner = new_event(test, f, sizeof(struct drm_event));

	KUNIT_EXPECT_EQ(test, atomic_read(&f->wakes), 0);
	KUNIT_EXPECT_EQ(test, priv->event_space, capacity - sizeof(struct drm_event));
	cancel_event(owner);
	KUNIT_EXPECT_EQ(test, atomic_read(&f->wakes), 1);
	KUNIT_EXPECT_EQ(test, priv->event_space, capacity);
	cancel_event(owner);
	KUNIT_EXPECT_EQ(test, atomic_read(&f->wakes), 1);
}

static void only_successful_reads_notify_space(struct kunit *test)
{
	struct space_fixture *f = new_fixture(test);
	struct drm_file *priv = f->file->private_data;
	unsigned long address = user_page(test);
	int capacity = priv->event_space;
	struct event_owner *owner = new_event(test, f, sizeof(struct drm_event));
	loff_t offset = 0;

	publish_event(owner);
	KUNIT_EXPECT_EQ(test, atomic_read(&f->wakes), 0);
	KUNIT_EXPECT_EQ(test, drm_read(f->file, (char __user *)address,
				      sizeof(struct drm_event) - 1, &offset), 0);
	KUNIT_EXPECT_EQ(test, atomic_read(&f->wakes), 0);
	KUNIT_EXPECT_EQ(test, priv->event_space, capacity - sizeof(struct drm_event));
	KUNIT_EXPECT_EQ(test, drm_read(f->file, (char __user *)1,
				      sizeof(struct drm_event), &offset), -EFAULT);
	KUNIT_EXPECT_EQ(test, atomic_read(&f->wakes), 0);
	KUNIT_EXPECT_EQ(test, priv->event_space, capacity - sizeof(struct drm_event));
	KUNIT_EXPECT_EQ(test, drm_read(f->file, (char __user *)address,
				      sizeof(struct drm_event), &offset), sizeof(struct drm_event));
	KUNIT_EXPECT_EQ(test, atomic_read(&f->wakes), 1);
	KUNIT_EXPECT_EQ(test, priv->event_space, capacity);
	KUNIT_EXPECT_EQ(test, drm_read(f->file, (char __user *)address,
				      sizeof(struct drm_event), &offset), -EAGAIN);
	KUNIT_EXPECT_EQ(test, atomic_read(&f->wakes), 1);
}

static void full_queue_can_retry_after_capacity_is_returned(struct kunit *test)
{
	struct space_fixture *f = new_fixture(test);
	struct drm_file *priv = f->file->private_data;
	struct test_event *extra;
	int capacity = priv->event_space;
	struct event_owner *owner = new_event(test, f, capacity);
	int ret;

	KUNIT_EXPECT_EQ(test, priv->event_space, 0);
	extra = kzalloc_obj(*extra);
	KUNIT_ASSERT_NOT_NULL(test, extra);
	extra->payload.type = DRM_EVENT_VBLANK;
	extra->payload.length = sizeof(extra->payload);
	ret = drm_event_reserve_init(f->dev, priv, &extra->pending, &extra->payload);
	if (ret)
		kfree(extra);
	else
		drm_event_cancel_free(f->dev, &extra->pending);
	KUNIT_EXPECT_EQ(test, ret, -ENOMEM);
	KUNIT_EXPECT_EQ(test, atomic_read(&f->wakes), 0);
	cancel_event(owner);
	KUNIT_EXPECT_EQ(test, atomic_read(&f->wakes), 1);
	owner = new_event(test, f, sizeof(struct drm_event));
	KUNIT_EXPECT_EQ(test, atomic_read(&f->wakes), 1);
	cancel_event(owner);
	KUNIT_EXPECT_EQ(test, atomic_read(&f->wakes), 2);
	KUNIT_EXPECT_EQ(test, priv->event_space, capacity);
}

static void cancellation_after_file_close_does_not_notify_freed_queue(struct kunit *test)
{
	struct space_fixture *f = new_fixture(test);
	struct event_owner *owner = new_event(test, f, sizeof(struct drm_event));

	kunit_release_action(test, detach_waiter, f);
	kunit_release_action(test, close_file, f->file);
	f->file = NULL;
	KUNIT_EXPECT_PTR_EQ(test, owner->event->pending.file_priv, NULL);
	cancel_event(owner);
	KUNIT_EXPECT_EQ(test, atomic_read(&f->wakes), 0);
}

static struct kunit_case event_space_cases[] = {
	KUNIT_CASE(cancellation_notifies_returned_capacity),
	KUNIT_CASE(only_successful_reads_notify_space),
	KUNIT_CASE(full_queue_can_retry_after_capacity_is_returned),
	KUNIT_CASE(cancellation_after_file_close_does_not_notify_freed_queue),
	{}
};

static struct kunit_suite event_space_suite = {
	.name = "drm_event_space",
	.test_cases = event_space_cases,
};
kunit_test_suite(event_space_suite);

MODULE_LICENSE("GPL and additional rights");
