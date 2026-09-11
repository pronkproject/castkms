// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/export.h>
#include <drm/drm_atomic.h>
#include <drm/drm_atomic_gamma.h>
#include <drm/drm_atomic_prepare_request.h>
#include <drm/drm_crtc.h>

struct gamma_request {
	struct drm_crtc *crtc;
	struct drm_property_blob *table;
	int (*validate)(struct drm_crtc *crtc, void *data);
	void *data;
};

static int build_gamma_update(struct drm_atomic_commit *state, void *data)
{
	struct gamma_request *request = data;
	int ret;

	ret = drm_modeset_lock_all_ctx(state->dev, state->acquire_ctx);
	if (ret)
		return ret;
	if (request->validate) {
		ret = request->validate(request->crtc, request->data);
		if (ret)
			return ret;
	}
	return drm_atomic_set_legacy_gamma(state, request->crtc, request->table);
}

/**
 * drm_atomic_commit_legacy_gamma - rebuild a blocking legacy gamma command
 * @crtc: caller-retained controller whose table changes
 * @table: caller-retained immutable blob of drm_color_lut entries
 * @owner: retained issuer whose revocation excludes installation
 * @validate: optional authorization callback on every attempt
 * @data: caller-owned validation data
 *
 * Call without modeset locks and retain the device configuration and arguments
 * until return. Each attempt checks authorization under modeset locks and
 * applies the same table to current controller state. Complete driver checking
 * precedes preparation. Attempted state and locks are dropped before waiting.
 * Only acceptance publishes the cached legacy readback values.
 *
 * The issuer grants no object access by itself. The caller supplies any
 * additional authorization policy through the callback.
 *
 * Return: zero on success or a negative error.
 */
int drm_atomic_commit_legacy_gamma(struct drm_crtc *crtc, struct drm_property_blob *table,
				   struct drm_prepare_owner *owner,
				   int (*validate)(struct drm_crtc *crtc, void *data), void *data)
{
	struct gamma_request request = { crtc, table, validate, data };

	if (!crtc || !table)
		return -EINVAL;
	return drm_atomic_commit_request_owned(crtc->dev, owner, build_gamma_update, &request);
}
EXPORT_SYMBOL_GPL(drm_atomic_commit_legacy_gamma);
