// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <drm/drm_constraints_list.h>
#include <drm/drm_constraints_output.h>
#include <drm/drm_constraints_query.h>
#include <drm/drm_crtc.h>
#include <drm/drm_drv.h>
#include <uapi/drm/drm_constraints.h>

#include "drm_constraints_uapi.h"

int drm_mode_list_constraints_ioctl(struct drm_device *dev, void *data,
				    struct drm_file *file)
{
	struct drm_mode_list_constraints *request = data;
	struct drm_constraints_list *list;
	struct drm_crtc *crtc;
	int ret;

	if (!drm_core_check_feature(dev, DRIVER_MODESET))
		return -EOPNOTSUPP;
	crtc = drm_crtc_find(dev, file, request->crtc_id);
	if (!crtc)
		return -ENOENT;
	list = drm_constraints_crtc_list(crtc);
	if (!list)
		return -EOPNOTSUPP;
	/* File/device ownership stabilizes topology; retain the list across faults. */
	drm_constraints_list_get(list);
	ret = drm_constraints_list_copy_to_user(list, request);
	drm_constraints_list_put(list);
	return ret;
}
