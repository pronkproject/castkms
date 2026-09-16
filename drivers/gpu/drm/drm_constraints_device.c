// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/err.h>
#include <linux/export.h>
#include <drm/drm_constraints_device.h>
#include <drm/drm_constraints_entry.h>
#include <drm/drm_device.h>
#include <drm/drm_drv.h>

int drm_constraints_device_init(struct drm_device *dev, unsigned int limit)
{
	struct drm_constraints_domain *domain;

	if (!drm_core_check_feature(dev, DRIVER_ATOMIC))
		return -EINVAL;
	if (dev->registered || dev->mode_config.num_crtc || dev->mode_config.constraints_domain)
		return -EBUSY;
	domain = drm_constraints_domain_create(limit);
	if (IS_ERR(domain))
		return PTR_ERR(domain);
	dev->mode_config.constraints_domain = domain;
	return 0;
}
EXPORT_SYMBOL_GPL(drm_constraints_device_init);

void drm_constraints_device_fini(struct drm_device *dev)
{
	if (dev->mode_config.constraints_domain) {
		drm_constraints_domain_put(dev->mode_config.constraints_domain);
		dev->mode_config.constraints_domain = NULL;
	}
}

struct drm_constraints_domain *drm_constraints_device_domain(struct drm_device *dev)
{
	return dev->mode_config.constraints_domain;
}
EXPORT_SYMBOL_GPL(drm_constraints_device_domain);
