// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <kunit/visibility.h>
#include <linux/export.h>
#include <drm/drm_atomic_request.h>
#include <drm/drm_auth.h>
#include <drm/drm_connector.h>
#include <drm/drm_crtc.h>
#include <drm/drm_file.h>
#include <drm/drm_lease.h>
#include <drm/drm_modeset_lock.h>
#include <drm/drm_plane.h>

#include "drm_atomic_user_request.h"

static int validate_object(struct drm_mode_object *object, struct drm_file *file)
{
	struct drm_device *dev;

	switch (object->type) {
	case DRM_MODE_OBJECT_CRTC:
		dev = obj_to_crtc(object)->dev;
		break;
	case DRM_MODE_OBJECT_PLANE:
		dev = obj_to_plane(object)->dev;
		break;
	case DRM_MODE_OBJECT_CONNECTOR:
		dev = obj_to_connector(object)->dev;
		if (drm_connector_is_unregistered(obj_to_connector(object)))
			return -ENOENT;
		break;
	default:
		return -EOPNOTSUPP;
	}
	if (dev != file->minor->dev)
		return -EINVAL;
	return drm_lease_held(file, object->id) ? 0 : -EACCES;
}

int drm_atomic_validate_user_request(const struct drm_atomic_user_request *request,
				     struct drm_file *file)
{
	const struct drm_atomic_request *values = drm_atomic_user_request_values(request);
	unsigned int i;
	int ret;

	if (!file || !drm_is_current_master(file))
		return -EACCES;
	drm_modeset_lock_assert_held(&file->minor->dev->mode_config.connection_mutex);
	for (i = 0; i < drm_atomic_user_request_target_count(request); i++) {
		ret = validate_object(drm_atomic_user_request_target(request, i), file);
		if (ret)
			return ret;
	}
	for (i = 0; i < drm_atomic_request_count(values); i++) {
		const struct drm_atomic_request_entry *entry = drm_atomic_request_entry(values, i);

		if (entry->type != DRM_ATOMIC_REQUEST_OBJECT || !entry->reference)
			continue;
		ret = validate_object(entry->reference, file);
		if (ret)
			return ret;
	}
	return 0;
}
EXPORT_SYMBOL_IF_KUNIT(drm_atomic_validate_user_request);
