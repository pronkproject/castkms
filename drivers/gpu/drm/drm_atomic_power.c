// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/export.h>
#include <drm/drm_atomic.h>
#include <drm/drm_atomic_power.h>
#include <drm/drm_connector.h>
#include <drm/drm_crtc.h>

/**
 * drm_atomic_set_connector_power - record a connector's power preference
 * @state: unchecked update on a preparation-enabled device
 * @connector: connector whose preference changes
 * @on: whether the connector requests an active controller
 *
 * The caller supplies an initialized modeset acquire context. The setter adds
 * the connector and its current controller to the attempt, then calculates
 * ACTIVE from all connectors assigned to that controller. A controller remains
 * active while any assigned connector requests it. Other connector preferences
 * are preserved; they are not inferred from the resulting controller activity.
 *
 * No accepted preference changes until installation. Multiple assignments in
 * one attempt use the latest pending preference for each connector. Clearing
 * the attempt discards those assignments. Complete atomic checks and authority
 * validation remain the caller's responsibility.
 * Set connector routing before assigning power preferences; if routing changes
 * afterward, rebuild the power assignments before checking the attempt.
 *
 * Return: zero on success or a negative error. Back off and rebuild the complete
 * attempt after a locking -EDEADLK.
 */
int drm_atomic_set_connector_power(struct drm_atomic_commit *state,
				   struct drm_connector *connector, bool on)
{
	struct drm_connector_state *connector_state, *other_state;
	struct drm_connector *other;
	struct drm_crtc_state *crtc_state;
	struct drm_crtc *crtc;
	int i, ret;
	bool active = false;

	if (state->dev != connector->dev || !state->acquire_ctx || state->checked)
		return -EINVAL;
	if (!state->dev->mode_config.preparation)
		return -EOPNOTSUPP;
	connector_state = drm_atomic_get_connector_state(state, connector);
	if (IS_ERR(connector_state))
		return PTR_ERR(connector_state);
	i = drm_connector_index(connector);
	state->connectors[i].update_power = true;
	state->connectors[i].power_on = on;
	crtc = connector_state->crtc;
	if (!crtc)
		return 0;
	ret = drm_atomic_add_affected_connectors(state, crtc);
	if (ret)
		return ret;
	crtc_state = drm_atomic_get_crtc_state(state, crtc);
	if (IS_ERR(crtc_state))
		return PTR_ERR(crtc_state);
	for_each_new_connector_in_state(state, other, other_state, i) {
		if (other_state->crtc != crtc)
			continue;
		if (state->connectors[i].update_power ? state->connectors[i].power_on :
		    other->dpms == DRM_MODE_DPMS_ON) {
			active = true;
			break;
		}
	}
	crtc_state->active = active;
	state->crtcs[drm_crtc_index(crtc)].power_from_connectors = true;
	return 0;
}
EXPORT_SYMBOL_GPL(drm_atomic_set_connector_power);

/**
 * drm_atomic_install_connector_power - publish accepted connector preferences
 * @state: accepted update whose connector and controller states are installed
 *
 * Call under the connection mutex during state installation, before dropping
 * modeset locks. Explicit connector preferences take precedence. Ordinary
 * atomic power changes and detachments update the legacy power value when no
 * connector preferences determined the controller's activity. Devices without
 * preparation retain their existing bookkeeping path.
 */
void drm_atomic_install_connector_power(struct drm_atomic_commit *state)
{
	struct drm_connector *connector;
	struct drm_connector_state *old_state, *new_state;
	int i;

	if (!state->dev->mode_config.preparation)
		return;
	for_each_oldnew_connector_in_state(state, connector, old_state, new_state, i) {
		struct drm_crtc *crtc = new_state->crtc;
		struct drm_crtc_state *crtc_state;

		drm_modeset_lock_assert_held(&state->dev->mode_config.connection_mutex);
		if (state->connectors[i].update_power) {
			connector->dpms = state->connectors[i].power_on ?
				DRM_MODE_DPMS_ON : DRM_MODE_DPMS_OFF;
			continue;
		}
		if (!crtc) {
			if (old_state->crtc)
				connector->dpms = DRM_MODE_DPMS_OFF;
			continue;
		}
		if (state->crtcs[drm_crtc_index(crtc)].power_from_connectors)
			continue;
		crtc_state = drm_atomic_get_new_crtc_state(state, crtc);
		if (crtc_state && drm_atomic_crtc_needs_modeset(crtc_state))
			connector->dpms = crtc_state->active ? DRM_MODE_DPMS_ON : DRM_MODE_DPMS_OFF;
	}
}
EXPORT_SYMBOL_GPL(drm_atomic_install_connector_power);
