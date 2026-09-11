// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/export.h>
#include <linux/overflow.h>
#include <drm/drm_atomic.h>
#include <drm/drm_atomic_gamma.h>
#include <drm/drm_atomic_uapi.h>
#include <drm/drm_color_mgmt.h>
#include <drm/drm_crtc.h>
#include <drm/drm_property.h>

#include "drm_crtc_internal.h"

/**
 * drm_atomic_set_legacy_gamma - record a legacy gamma table in an attempt
 * @state: unchecked update with an initialized modeset acquire context
 * @crtc: controller whose legacy table changes
 * @table: retained blob containing exactly gamma_size drm_color_lut entries
 *
 * Requires preparation support and standard atomic color properties rather
 * than a driver-specific gamma_set callback. Selects GAMMA_LUT when available,
 * otherwise DEGAMMA_LUT, and clears the other table and color matrix. The
 * ordinary color setter validates the selected property's advertised size;
 * full driver validation remains the caller's responsibility.
 *
 * The attempt retains its own reference for legacy readback independently of
 * the pending color properties. Later atomic color assignments do not change
 * what the legacy command requested. Installation publishes the legacy table
 * under the controller lock. Rejection and clearing leave readback unchanged.
 * The caller retains its reference and must not modify the blob's contents.
 *
 * Return: zero on success or a negative error. A locking -EDEADLK requires
 * clearing and rebuilding the complete attempt after backing off.
 */
int drm_atomic_set_legacy_gamma(struct drm_atomic_commit *state,
				struct drm_crtc *crtc, struct drm_property_blob *table)
{
	struct drm_mode_config *config = &state->dev->mode_config;
	struct drm_crtc_state *crtc_state;
	struct drm_property *property;
	size_t bytes;
	bool use_gamma, changed;
	int ret;

	if (crtc->dev != state->dev || !state->acquire_ctx || state->checked || !table)
		return -EINVAL;
	if (!config->preparation || crtc->funcs->gamma_set)
		return -EOPNOTSUPP;
	if (!crtc->gamma_size || !crtc->gamma_store ||
	    check_mul_overflow((size_t)crtc->gamma_size, sizeof(struct drm_color_lut), &bytes) ||
	    table->length != bytes || table->dev != state->dev)
		return -EINVAL;
	use_gamma = drm_mode_obj_find_prop_id(&crtc->base, config->gamma_lut_property->base.id);
	property = use_gamma ? config->gamma_lut_property : config->degamma_lut_property;
	if (!drm_mode_obj_find_prop_id(&crtc->base, property->base.id))
		return -ENODEV;
	crtc_state = drm_atomic_get_crtc_state(state, crtc);
	if (IS_ERR(crtc_state))
		return PTR_ERR(crtc_state);
	ret = drm_atomic_set_color_property_for_crtc(crtc_state, property, table);
	if (ret)
		return ret;
	changed = drm_property_replace_blob(use_gamma ? &crtc_state->degamma_lut :
						       &crtc_state->gamma_lut, NULL);
	changed |= drm_property_replace_blob(&crtc_state->ctm, NULL);
	crtc_state->color_mgmt_changed |= changed;
	drm_property_replace_blob(&state->crtcs[drm_crtc_index(crtc)].legacy_gamma, table);
	return 0;
}
EXPORT_SYMBOL_GPL(drm_atomic_set_legacy_gamma);
