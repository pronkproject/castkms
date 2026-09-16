// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <drm/drm_atomic.h>
#include <drm/drm_atomic_constraints.h>
#include <drm/drm_atomic_prepare_request.h>
#include <drm/drm_atomic_uapi.h>
#include <drm/drm_constraints_device.h>
#include <drm/drm_constraints_list.h>
#include <drm/drm_constraints_output.h>
#include <drm/drm_device.h>

static int build_quiesce(struct drm_atomic_commit *state, void *data)
{
	struct drm_connector_state *connector_state;
	struct drm_connector *connector;
	struct drm_plane_state *plane_state;
	struct drm_plane *plane;
	struct drm_crtc_state *crtc_state;
	struct drm_crtc *crtc;
	bool found = false;
	int i, ret;

	drm_for_each_crtc(crtc, state->dev) {
		if (!drm_constraints_crtc_list(crtc))
			continue;
		found = true;
		crtc_state = drm_atomic_get_crtc_state(state, crtc);
		if (IS_ERR(crtc_state))
			return PTR_ERR(crtc_state);
		crtc_state->active = false;
		ret = drm_atomic_set_mode_prop_for_crtc(crtc_state, NULL);
		if (ret)
			return ret;
		ret = drm_atomic_add_affected_planes(state, crtc);
		if (ret)
			return ret;
		ret = drm_atomic_add_affected_connectors(state, crtc);
		if (ret)
			return ret;
	}
	for_each_new_connector_in_state(state, connector, connector_state, i) {
		ret = drm_atomic_set_crtc_for_connector(connector_state, NULL);
		if (ret)
			return ret;
	}
	for_each_new_plane_in_state(state, plane, plane_state, i) {
		ret = drm_atomic_set_crtc_for_plane(plane_state, NULL);
		if (ret)
			return ret;
		drm_atomic_set_fb_for_plane(plane_state, NULL);
	}
	return found ? 0 : DRM_ATOMIC_REQUEST_UNCHANGED;
}

int drm_constraints_recover(struct drm_device *dev)
{
	struct drm_crtc *crtc;
	int ret;

	if (!drm_constraints_device_domain(dev))
		return -EOPNOTSUPP;
	ret = drm_atomic_commit_request(dev, build_quiesce, NULL);
	if (ret)
		return ret;
	drm_for_each_crtc(crtc, dev) {
		if (!drm_constraints_crtc_list(crtc))
			continue;
		ret = drm_atomic_constraints_restore_default(crtc);
		if (ret)
			return ret;
	}
	drm_for_each_crtc(crtc, dev) {
		struct drm_constraints_list *list = drm_constraints_crtc_list(crtc);

		if (!list)
			continue;
		ret = drm_constraints_list_retain_default(list, drm_constraints_crtc_default(crtc));
		if (ret)
			return ret;
	}
	return 0;
}
EXPORT_SYMBOL_GPL(drm_constraints_recover);
