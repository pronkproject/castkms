// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <drm/drm_atomic_power.h>
#include <drm/drm_atomic_prepare_auth.h>
#include <drm/drm_atomic_prepare_owner.h>
#include <drm/drm_auth.h>
#include <drm/drm_connector.h>
#include <drm/drm_crtc.h>
#include <drm/drm_file.h>
#include <drm/drm_lease.h>

#include "drm_atomic_user_commit.h"

struct user_power {
	struct drm_file *file;
	bool on;
};

static int validate_power(struct drm_connector *connector, void *data)
{
	struct user_power *request = data;
	struct drm_crtc *crtc = connector->state->crtc;

	if (request->on && drm_connector_is_unregistered(connector))
		return -ENOENT;
	if (!drm_is_current_master(request->file) ||
	    !drm_lease_held(request->file, connector->base.id) ||
	    (crtc && !drm_lease_held(request->file, crtc->base.id)))
		return -EACCES;
	return 0;
}

int drm_atomic_commit_user_power(struct drm_connector *connector, u64 mode, struct drm_file *file)
{
	struct user_power request = { file, mode == DRM_MODE_DPMS_ON };
	struct drm_prepare_owner *owner;
	int ret;

	if (mode > DRM_MODE_DPMS_OFF)
		return -EINVAL;
	owner = drm_file_prepare_owner(file);
	if (IS_ERR(owner))
		return PTR_ERR(owner);
	ret = drm_atomic_commit_connector_power(connector, request.on, owner,
					       validate_power, &request);
	drm_prepare_owner_put(owner);
	return ret;
}
