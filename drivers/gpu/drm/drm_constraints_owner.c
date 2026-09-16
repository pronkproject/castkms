// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/slab.h>
#include <linux/workqueue.h>
#include <kunit/visibility.h>
#include <drm/drm_constraints_device.h>
#include <drm/drm_constraints_owner.h>
#include <drm/drm_device.h>
#include <drm/drm_drv.h>

enum owner_state {
	OWNER_READY,
	OWNER_PENDING,
	OWNER_FAILED,
	OWNER_CLOSED,
};

struct drm_constraints_owner {
	struct drm_device *dev;
	struct work_struct work;
	/* State and error are serialized by the device's master_mutex. */
	enum owner_state state;
	int error;
};

static void recover_owner(struct work_struct *work)
{
	struct drm_constraints_owner *owner = container_of(work, typeof(*owner), work);
	struct drm_device *dev = owner->dev;
	int index, ret;

	mutex_lock(&dev->master_mutex);
	ret = owner->state == OWNER_CLOSED ? -ENODEV : dev->master ? -EACCES : 0;
	mutex_unlock(&dev->master_mutex);
	if (!ret) {
		if (drm_dev_enter(dev, &index)) {
			ret = drm_constraints_recover(dev);
			drm_dev_exit(index);
		} else {
			ret = -ENODEV;
		}
	}
	mutex_lock(&dev->master_mutex);
	if (owner->state != OWNER_CLOSED) {
		owner->error = ret;
		owner->state = ret ? OWNER_FAILED : OWNER_READY;
	}
	mutex_unlock(&dev->master_mutex);
	/* Final device release may free owner; no access follows this put. */
	drm_dev_put(dev);
}

int drm_constraints_owner_init(struct drm_device *dev)
{
	struct drm_constraints_owner *owner = kzalloc_obj(*owner);

	if (!owner)
		return -ENOMEM;
	owner->dev = dev;
	owner->state = OWNER_READY;
	INIT_WORK(&owner->work, recover_owner);
	dev->mode_config.constraints_owner = owner;
	return 0;
}

static void queue_recovery(struct drm_constraints_owner *owner)
{
	lockdep_assert_held(&owner->dev->master_mutex);
	owner->state = OWNER_PENDING;
	owner->error = 0;
	drm_dev_get(owner->dev);
	if (WARN_ON_ONCE(!schedule_work(&owner->work))) {
		owner->state = OWNER_FAILED;
		owner->error = -EIO;
		drm_dev_put(owner->dev);
	}
}

void drm_constraints_owner_lost(struct drm_device *dev)
{
	struct drm_constraints_owner *owner = dev->mode_config.constraints_owner;

	lockdep_assert_held(&dev->master_mutex);
	if (owner && (owner->state == OWNER_READY || owner->state == OWNER_FAILED))
		queue_recovery(owner);
}
EXPORT_SYMBOL_GPL(drm_constraints_owner_lost);

int drm_constraints_owner_check(struct drm_device *dev)
{
	struct drm_constraints_owner *owner = dev->mode_config.constraints_owner;

	lockdep_assert_held(&dev->master_mutex);
	if (!owner || owner->state == OWNER_READY)
		return 0;
	if (owner->state == OWNER_CLOSED)
		return -ENODEV;
	return owner->state == OWNER_PENDING ? -EBUSY : owner->error;
}
EXPORT_SYMBOL_GPL(drm_constraints_owner_check);

void drm_constraints_owner_retry(struct drm_device *dev)
{
	struct drm_constraints_owner *owner = dev->mode_config.constraints_owner;

	lockdep_assert_held(&dev->master_mutex);
	if (owner && owner->state == OWNER_FAILED)
		queue_recovery(owner);
}
EXPORT_SYMBOL_GPL(drm_constraints_owner_retry);

void drm_constraints_owner_flush(struct drm_device *dev)
{
	struct drm_constraints_owner *owner = dev->mode_config.constraints_owner;

	if (owner)
		flush_work(&owner->work);
}
EXPORT_SYMBOL_GPL(drm_constraints_owner_flush);

void drm_constraints_owner_stop(struct drm_device *dev)
{
	struct drm_constraints_owner *owner = dev->mode_config.constraints_owner;

	if (!owner)
		return;
	mutex_lock(&dev->master_mutex);
	owner->state = OWNER_CLOSED;
	mutex_unlock(&dev->master_mutex);
	/* The worker's final device put may invoke managed mode-config cleanup. */
	if (current_work() != &owner->work && cancel_work_sync(&owner->work))
		drm_dev_put(dev);
}
EXPORT_SYMBOL_IF_KUNIT(drm_constraints_owner_stop);

void drm_constraints_owner_fini(struct drm_device *dev)
{
	kfree(dev->mode_config.constraints_owner);
	dev->mode_config.constraints_owner = NULL;
}
