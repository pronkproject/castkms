// SPDX-License-Identifier: GPL-2.0 OR MIT
/* Provider issuance stays separate from descriptor publication and pixel access. */

#include <linux/err.h>
#include <linux/file.h>
#include <linux/module.h>

#include <drm/drm_capture_file.h>
#include <drm/drm_capture_grant.h>
#include <drm/drm_device.h>
#include <drm/drm_drv.h>
#include <drm/drm_file.h>
#include <drm/drm_mode_config.h>

void drm_capture_files_put(struct drm_capture_files *files)
{
	struct file *capture = files->capture;
	struct file *control = files->control;

	files->capture = NULL;
	files->control = NULL;
	if (!IS_ERR_OR_NULL(capture))
		fput(capture);
	if (!IS_ERR_OR_NULL(control))
		fput(control);
}
EXPORT_SYMBOL_GPL(drm_capture_files_put);

int drm_capture_create_file_grant(struct drm_device *dev, struct drm_file *file,
				  const struct drm_capture_target *target,
				  u32 flags,
				  struct drm_capture_files *result)
{
	struct drm_capture_files created = {};
	const struct drm_mode_config_funcs *funcs;
	int ret;

	if (!file || !file->minor || file->minor->dev != dev ||
	    !target || !target->crtc_id || !target->connector_id)
		return -EINVAL;
	if (!READ_ONCE(dev->registered) || drm_dev_is_unplugged(dev))
		return -ENODEV;
	if (!drm_core_check_feature(dev, DRIVER_MODESET))
		return -EOPNOTSUPP;
	funcs = dev->mode_config.funcs;
	if (!funcs || !funcs->create_capture_grant)
		return -EOPNOTSUPP;
	if (flags & ~funcs->capture_grant_flags)
		return -EOPNOTSUPP;
	ret = funcs->create_capture_grant(dev, file, target, flags, &created);
	if (ret > 0)
		ret = -EINVAL;
	if (!ret && (IS_ERR_OR_NULL(created.capture) || IS_ERR_OR_NULL(created.control) ||
		     !drm_capture_files_match(created.capture, created.control)))
		ret = -EINVAL;
	if (ret) {
		drm_capture_files_put(&created);
		return ret;
	}
	*result = created;
	return 0;
}
EXPORT_SYMBOL_GPL(drm_capture_create_file_grant);
