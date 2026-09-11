// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/export.h>
#include <linux/slab.h>
#include <drm/drm_atomic_request.h>
#include <drm/drm_colorop.h>
#include <drm/drm_connector.h>
#include <drm/drm_crtc.h>
#include <drm/drm_property.h>

#include "drm_crtc_internal.h"

struct drm_atomic_request {
	unsigned int count;
	struct drm_atomic_request_entry entries[] __counted_by(count);
};

static struct drm_device *object_device(struct drm_mode_object *object)
{
	switch (object->type) {
	case DRM_MODE_OBJECT_CRTC:
		return obj_to_crtc(object)->dev;
	case DRM_MODE_OBJECT_PLANE:
		return obj_to_plane(object)->dev;
	case DRM_MODE_OBJECT_CONNECTOR:
		return obj_to_connector(object)->dev;
	case DRM_MODE_OBJECT_COLOROP:
		return obj_to_colorop(object)->dev;
	default:
		return NULL;
	}
}

static int validate_entry(struct drm_device *dev,
			  const struct drm_atomic_request_entry *entry)
{
	struct drm_property *prop = entry->property;
	struct drm_mode_object *unused;

	if (!entry->object || !prop || prop->dev != dev ||
	    object_device(entry->object) != dev || !entry->object->properties ||
	    drm_mode_obj_find_prop_id(entry->object, prop->base.id) != prop ||
	    prop->flags & DRM_MODE_PROP_IMMUTABLE)
		return -EINVAL;
	if (prop == dev->mode_config.prop_prepare_fd ||
	    prop == dev->mode_config.prop_out_fence_ptr ||
	    prop == dev->mode_config.writeback_out_fence_ptr_property)
		return -EOPNOTSUPP;
	if (prop == dev->mode_config.prop_in_fence_fd)
		return -EINVAL;
	if (drm_property_type_is(prop, DRM_MODE_PROP_BLOB))
		return -EINVAL;
	if (drm_property_type_is(prop, DRM_MODE_PROP_OBJECT)) {
		return -EINVAL;
	}
	if (entry->type != DRM_ATOMIC_REQUEST_SCALAR)
		return -EINVAL;
	return drm_property_change_valid_get(prop, entry->scalar, &unused) ? 0 : -EINVAL;
}

static void retain_entry(const struct drm_atomic_request_entry *entry)
{
	drm_mode_object_get(entry->object);
}

struct drm_atomic_request *
drm_atomic_request_create(struct drm_device *dev,
			  const struct drm_atomic_request_entry *entries,
			  unsigned int count)
{
	struct drm_atomic_request *request;
	unsigned int i;
	int ret;

	if (!dev || (count && !entries))
		return ERR_PTR(-EINVAL);
	for (i = 0; i < count; i++) {
		ret = validate_entry(dev, &entries[i]);
		if (ret)
			return ERR_PTR(ret);
	}
	request = kvzalloc(struct_size(request, entries, count), GFP_KERNEL);
	if (!request)
		return ERR_PTR(-ENOMEM);
	request->count = count;
	for (i = 0; i < count; i++) {
		request->entries[i] = entries[i];
		retain_entry(&request->entries[i]);
	}
	return request;
}
EXPORT_SYMBOL_GPL(drm_atomic_request_create);

void drm_atomic_request_destroy(struct drm_atomic_request *request)
{
	unsigned int i;

	if (!request)
		return;
	for (i = 0; i < request->count; i++) {
		const struct drm_atomic_request_entry *entry = &request->entries[i];

		drm_mode_object_put(entry->object);
	}
	kvfree(request);
}
EXPORT_SYMBOL_GPL(drm_atomic_request_destroy);

unsigned int drm_atomic_request_count(const struct drm_atomic_request *request)
{
	return request->count;
}
EXPORT_SYMBOL_GPL(drm_atomic_request_count);

const struct drm_atomic_request_entry *
drm_atomic_request_entry(const struct drm_atomic_request *request, unsigned int index)
{
	return index < request->count ? &request->entries[index] : NULL;
}
EXPORT_SYMBOL_GPL(drm_atomic_request_entry);
