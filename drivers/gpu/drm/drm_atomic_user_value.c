// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <kunit/visibility.h>
#include <linux/dma-fence.h>
#include <linux/export.h>
#include <linux/sync_file.h>
#include <drm/drm_device.h>
#include <drm/drm_framebuffer.h>
#include <drm/drm_lease.h>
#include <drm/drm_property.h>

#include "drm_atomic_request_internal.h"
#include "drm_atomic_user_value.h"
#include "drm_crtc_internal.h"

void drm_atomic_release_user_value(struct drm_atomic_request_entry *entry)
{
	switch (entry->type) {
	case DRM_ATOMIC_REQUEST_FRAMEBUFFER:
		if (entry->framebuffer)
			drm_framebuffer_put(entry->framebuffer);
		break;
	case DRM_ATOMIC_REQUEST_BLOB:
		drm_property_blob_put(entry->blob);
		break;
	case DRM_ATOMIC_REQUEST_OBJECT:
		if (entry->reference)
			drm_mode_object_put(entry->reference);
		break;
	case DRM_ATOMIC_REQUEST_FENCE:
		dma_fence_put(entry->fence);
		break;
	case DRM_ATOMIC_REQUEST_SCALAR:
		break;
	}
}
EXPORT_SYMBOL_IF_KUNIT(drm_atomic_release_user_value);

int drm_atomic_resolve_user_value(struct drm_mode_object *object, struct drm_property *property,
				  struct drm_file *file, u64 value,
				  struct drm_atomic_request_entry *entry)
{
	struct drm_atomic_request_entry resolved = { .object = object, .property = property };
	struct drm_mode_object *reference;

	if (!object->properties || drm_mode_obj_find_prop_id(object, property->base.id) != property)
		return -EINVAL;
	if (!drm_atomic_request_supports_property(object, property))
		return -EOPNOTSUPP;
	if (!drm_property_change_valid_get(property, value, &reference))
		return -EINVAL;

	if (property == property->dev->mode_config.prop_in_fence_fd) {
		resolved.type = DRM_ATOMIC_REQUEST_FENCE;
		if (value != U64_MAX) {
			resolved.fence = sync_file_get_fence(value);
			if (!resolved.fence)
				return -EINVAL;
		}
	} else if (drm_property_type_is(property, DRM_MODE_PROP_BLOB)) {
		resolved.type = DRM_ATOMIC_REQUEST_BLOB;
		resolved.blob = reference ? obj_to_blob(reference) : NULL;
	} else if (drm_property_type_is(property, DRM_MODE_PROP_OBJECT)) {
		if (reference && drm_mode_object_lease_required(reference->type) &&
		    !drm_lease_held(file, reference->id)) {
			drm_mode_object_put(reference);
			return -EACCES;
		}
		if (property->values[0] == DRM_MODE_OBJECT_FB) {
			resolved.type = DRM_ATOMIC_REQUEST_FRAMEBUFFER;
			resolved.framebuffer = reference ? obj_to_fb(reference) : NULL;
		} else {
			resolved.type = DRM_ATOMIC_REQUEST_OBJECT;
			resolved.reference = reference;
		}
	} else {
		resolved.type = DRM_ATOMIC_REQUEST_SCALAR;
		resolved.scalar = value;
	}
	*entry = resolved;
	return 0;
}
EXPORT_SYMBOL_IF_KUNIT(drm_atomic_resolve_user_value);
