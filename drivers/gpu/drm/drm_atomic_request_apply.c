// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/export.h>
#include <drm/drm_atomic.h>
#include <drm/drm_atomic_request.h>
#include <drm/drm_atomic_uapi.h>
#include <drm/drm_property.h>

#include "drm_crtc_internal.h"

static bool supported_entry(const struct drm_atomic_request_entry *entry)
{
	struct drm_mode_config *config = &entry->property->dev->mode_config;

	if (entry->object->type == DRM_MODE_OBJECT_PLANE)
		return entry->property == config->prop_fb_id ||
		       entry->property == config->prop_in_fence_fd;
	return false;
}

static int apply_plane(struct drm_atomic_commit *state,
		       const struct drm_atomic_request_entry *entry)
{
	struct drm_mode_config *config = &state->dev->mode_config;
	struct drm_plane_state *plane_state;

	plane_state = drm_atomic_get_plane_state(state, obj_to_plane(entry->object));
	if (IS_ERR(plane_state))
		return PTR_ERR(plane_state);
	if (entry->property == config->prop_fb_id) {
		drm_atomic_set_fb_for_plane(plane_state, entry->framebuffer);
		return 0;
	}
	if (entry->property == config->prop_in_fence_fd)
		return drm_atomic_set_fence_for_plane(plane_state, entry->fence);
	return -EOPNOTSUPP;
}

static int apply_entry(struct drm_atomic_commit *state,
		       const struct drm_atomic_request_entry *entry)
{
	switch (entry->object->type) {
	case DRM_MODE_OBJECT_PLANE:
		return apply_plane(state, entry);
	default:
		return -EOPNOTSUPP;
	}
}

/**
 * drm_atomic_request_apply - apply retained values to an uncommitted update
 * @request: immutable request retaining the input references
 * @state: fresh or cleared atomic update with an initialized acquire context
 * @validate: required caller authority and availability check
 * @data: caller data for @validate
 *
 * Acquires all modeset locks, verifies property attachment and the supported
 * property set, then calls @validate before applying any assignment. The caller
 * must check permission for all targets and referenced objects, including any
 * unregistered objects, on every invocation. Retention does not grant access.
 * The callback runs under modeset locks and must not drop them or modify the
 * request or update. The caller keeps authority valid through submission.
 *
 * Entries are applied in order using kernel references, without identifier or
 * descriptor lookup. Supported properties are plane FB_ID, IN_FENCE_FD.
 * Driver-private properties and asynchronous-flip validation are not supported.
 * No check or commit runs.
 *
 * Locks remain in the caller's acquire context on every return. On error the
 * caller must clear or discard @state before retrying; assignments are not
 * rolled back. Neither @request nor its references are consumed.
 *
 * Return: 0 on success; -EOPNOTSUPP for unsupported properties; otherwise a
 * negative validation, locking or setter error. Handle -EDEADLK by clearing
 * @state and backing off its acquire context before another attempt.
 */
int drm_atomic_request_apply(const struct drm_atomic_request *request,
			     struct drm_atomic_commit *state,
			     int (*validate)(struct drm_atomic_commit *state,
					     const struct drm_atomic_request *request,
					     void *data),
			     void *data)
{
	unsigned int i;
	int ret;

	if (!state->acquire_ctx || !validate || state->async_update)
		return -EINVAL;
	ret = drm_modeset_lock_all_ctx(state->dev, state->acquire_ctx);
	if (ret)
		return ret;
	for (i = 0; i < drm_atomic_request_count(request); i++) {
		const struct drm_atomic_request_entry *entry = drm_atomic_request_entry(request, i);

		if (entry->property->dev != state->dev ||
		    drm_mode_obj_find_prop_id(entry->object, entry->property->base.id) !=
			entry->property)
			return -EINVAL;
		if (!supported_entry(entry))
			return -EOPNOTSUPP;
	}
	ret = validate(state, request, data);
	if (ret)
		return ret;
	for (i = 0; i < drm_atomic_request_count(request); i++) {
		ret = apply_entry(state, drm_atomic_request_entry(request, i));
		if (ret)
			return ret;
	}
	return 0;
}
EXPORT_SYMBOL_GPL(drm_atomic_request_apply);
