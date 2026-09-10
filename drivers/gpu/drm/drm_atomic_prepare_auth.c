// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/err.h>
#include <linux/export.h>
#include <drm/drm_atomic_prepare_auth.h>
#include <drm/drm_atomic_prepare_owner.h>
#include <drm/drm_auth.h>
#include <drm/drm_device.h>
#include <drm/drm_drv.h>
#include <drm/drm_file.h>
#include <drm/drm_lease.h>

/* Retained terminal descriptors count toward the issuer's allocation bound. */
#define DRM_PREPARE_ISSUER_MAX_TICKETS 256

struct drm_prepare_owner *drm_file_prepare_owner(struct drm_file *file)
{
	struct drm_device *dev = file->minor->dev;
	struct drm_prepare_owner *owner;
	struct drm_master *master;

	if (!drm_core_check_feature(dev, DRIVER_MODESET))
		return ERR_PTR(-EOPNOTSUPP);
	guard(mutex)(&dev->master_mutex);
	master = file->master;
	if (!file->is_master || !master || drm_lease_owner(master) != dev->master)
		return ERR_PTR(-EACCES);
	guard(mutex)(&dev->mode_config.idr_mutex);
	owner = master->prepare_owner;
	if (!owner) {
		owner = drm_prepare_owner_create(DRM_PREPARE_ISSUER_MAX_TICKETS);
		if (IS_ERR(owner))
			return owner;
		master->prepare_owner = owner;
	}
	return drm_prepare_owner_get(owner);
}
EXPORT_SYMBOL_GPL(drm_file_prepare_owner);

void drm_master_cancel_preparation_locked(struct drm_master *master)
{
	struct drm_prepare_owner *owner = master->prepare_owner;

	lockdep_assert_held(&master->dev->mode_config.idr_mutex);
	if (!owner)
		return;
	drm_prepare_owner_revoke(owner);
	master->prepare_owner = NULL;
	drm_prepare_owner_put(owner);
}

void drm_master_cancel_preparation(struct drm_master *top)
{
	struct drm_master *master = top;

	guard(mutex)(&top->dev->mode_config.idr_mutex);
	for (;;) {
		drm_master_cancel_preparation_locked(master);
		if (!list_empty(&master->lessees)) {
			master = list_first_entry(&master->lessees, struct drm_master, lessee_list);
			continue;
		}
		while (master != top && list_is_last(&master->lessee_list, &master->lessor->lessees))
			master = master->lessor;
		if (master == top)
			break;
		master = list_next_entry(master, lessee_list);
	}
}

void drm_file_cancel_preparation(struct drm_file *file)
{
	struct drm_device *dev = file->minor->dev;

	if (!drm_core_check_feature(dev, DRIVER_MODESET))
		return;
	guard(mutex)(&dev->master_mutex);
	if (file->is_master && file->master)
		drm_master_cancel_preparation(file->master);
}
