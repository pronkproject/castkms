// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/err.h>
#include <linux/export.h>
#include <drm/drm_constraints_device.h>
#include <drm/drm_constraints_entry.h>
#include <drm/drm_constraints_owner.h>
#include <drm/drm_device.h>
#include <drm/drm_drv.h>
#include <drm/drm_property.h>
#include <uapi/drm/drm_constraints.h>

int drm_constraints_device_init(struct drm_device *dev, unsigned int limit)
{
	struct drm_constraints_domain *domain;
	struct drm_property *property;
	int ret;

	if (!drm_core_check_feature(dev, DRIVER_ATOMIC))
		return -EINVAL;
	if (dev->registered || dev->mode_config.num_crtc || dev->mode_config.constraints_domain)
		return -EBUSY;
	domain = drm_constraints_domain_create(limit);
	if (IS_ERR(domain))
		return PTR_ERR(domain);
	ret = drm_constraints_owner_init(dev);
	if (ret) {
		drm_constraints_domain_put(domain);
		return ret;
	}
	property = drm_property_create_range(dev, DRM_MODE_PROP_ATOMIC,
					     DRM_CONSTRAINTS_ID_PROPERTY, 1, U64_MAX);
	if (!property) {
		drm_constraints_owner_fini(dev);
		drm_constraints_domain_put(domain);
		return -ENOMEM;
	}
	dev->mode_config.prop_constraints_id = property;
	dev->mode_config.constraints_domain = domain;
	return 0;
}
EXPORT_SYMBOL_GPL(drm_constraints_device_init);

void drm_constraints_device_fini(struct drm_device *dev)
{
	/* Mode-config cleanup owns property destruction. */
	dev->mode_config.prop_constraints_id = NULL;
	drm_constraints_owner_fini(dev);
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
