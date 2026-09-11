// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/export.h>
#include <drm/drm_atomic.h>
#include <drm/drm_atomic_power.h>
#include <drm/drm_atomic_prepare_request.h>
#include <drm/drm_connector.h>

struct power_request {
	struct drm_connector *connector;
	bool on;
	int (*validate)(struct drm_connector *connector, void *data);
	void *data;
};

static int build_power_update(struct drm_atomic_commit *state, void *data)
{
	struct power_request *request = data;
	int ret;

	ret = drm_modeset_lock_all_ctx(state->dev, state->acquire_ctx);
	if (ret)
		return ret;
	if (request->validate) {
		ret = request->validate(request->connector, request->data);
		if (ret)
			return ret;
	}
	return drm_atomic_set_connector_power(state, request->connector, request->on);
}

/**
 * drm_atomic_commit_connector_power - rebuild a connector power command
 * @connector: caller-retained connector whose preference changes
 * @on: whether the connector requests an active controller
 * @owner: retained issuer whose revocation excludes installation
 * @validate: optional authorization callback on every attempt
 * @data: caller-owned validation data
 *
 * Call without modeset locks and retain the connector, device configuration and
 * validation data until return. Each attempt obtains all modeset locks, checks
 * authorization and calculates controller activity from the connector's current
 * routing and other connectors' current preferences. The command's requested
 * preference remains unchanged across waits.
 *
 * Complete atomic validation precedes preparation. Attempted states and locks
 * are dropped before waiting for readers. The issuer binds final acceptance;
 * any additional authority remains the caller's responsibility.
 *
 * Returns: zero on success or a negative error.
 */
int drm_atomic_commit_connector_power(struct drm_connector *connector, bool on,
				      struct drm_prepare_owner *owner,
				      int (*validate)(struct drm_connector *connector, void *data),
				      void *data)
{
	struct power_request request = { connector, on, validate, data };

	if (!connector)
		return -EINVAL;
	return drm_atomic_commit_request_owned(connector->dev, owner, build_power_update, &request);
}
EXPORT_SYMBOL_GPL(drm_atomic_commit_connector_power);
