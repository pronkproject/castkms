// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <drm/drm_atomic_prepare_auth.h>
#include <drm/drm_atomic_prepare_owner.h>
#include <drm/drm_mode_object.h>
#include <drm/drm_property.h>

#include "drm_atomic_user_commit.h"
#include "drm_atomic_user_input.h"

int drm_atomic_commit_user_property(struct drm_mode_object *object,
				    struct drm_property *property, u64 value,
				    struct drm_file *file)
{
	u32 object_id = object->id, property_id = property->base.id, count = 1;
	const struct drm_atomic_user_input input = {
		.object_count = 1, .property_count = 1,
		.objects = &object_id, .counts = &count,
		.properties = &property_id, .values = &value,
	};
	struct drm_prepare_owner *owner = drm_file_prepare_owner(file);
	int ret;

	if (IS_ERR(owner))
		return PTR_ERR(owner);
	ret = drm_atomic_commit_user_request(property->dev, file, owner,
					     DRM_MODE_ATOMIC_ALLOW_MODESET, 0, &input);
	drm_prepare_owner_put(owner);
	return ret;
}
