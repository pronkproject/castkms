// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <kunit/visibility.h>
#include <linux/export.h>
#include <linux/slab.h>
#include <drm/drm_atomic.h>
#include <drm/drm_atomic_request.h>
#include <drm/drm_mode_object.h>
#include <drm/drm_modeset_lock.h>

#include "drm_atomic_user_input.h"
#include "drm_atomic_user_request.h"
#include "drm_atomic_user_value.h"
#include "drm_crtc_internal.h"

struct drm_atomic_user_request {
	struct drm_atomic_request *values;
	unsigned int target_count;
	struct drm_mode_object *targets[] __counted_by(target_count);
};

void drm_atomic_free_user_request(struct drm_atomic_user_request *request)
{
	unsigned int i;

	if (!request)
		return;
	drm_atomic_request_destroy(request->values);
	for (i = 0; i < request->target_count; i++) {
		if (request->targets[i])
			drm_mode_object_put(request->targets[i]);
	}
	kvfree(request);
}
EXPORT_SYMBOL_IF_KUNIT(drm_atomic_free_user_request);

struct drm_atomic_user_request *
drm_atomic_resolve_user_request(struct drm_device *dev, struct drm_file *file,
				const struct drm_atomic_user_input *input)
{
	struct drm_atomic_user_request *request;
	struct drm_atomic_request_entry *entries;
	unsigned int i, j, resolved = 0;
	int ret;

	drm_modeset_lock_assert_held(&dev->mode_config.connection_mutex);
	request = kvzalloc(struct_size(request, targets, input->object_count), GFP_KERNEL);
	if (!request)
		return ERR_PTR(-ENOMEM);
	request->target_count = input->object_count;
	entries = kvcalloc(input->property_count, sizeof(*entries), GFP_KERNEL);
	if (!entries) {
		ret = -ENOMEM;
		goto fail;
	}
	for (i = 0; i < input->object_count; i++) {
		struct drm_mode_object *object;

		object = drm_mode_object_find(dev, file, input->objects[i], DRM_MODE_OBJECT_ANY);
		request->targets[i] = object;
		if (!object || !object->properties) {
			ret = -ENOENT;
			goto release_values;
		}
		/* The count check also protects kernel-constructed input descriptors. */
		if (input->counts[i] > input->property_count - resolved) {
			ret = -EINVAL;
			goto release_values;
		}
		for (j = 0; j < input->counts[i]; j++) {
			struct drm_property *property;

			property = drm_mode_obj_find_prop_id(object, input->properties[resolved]);
			if (!property) {
				ret = -ENOENT;
				goto release_values;
			}
			ret = drm_atomic_resolve_user_value(object, property, file,
							    input->values[resolved], &entries[resolved]);
			if (ret)
				goto release_values;
			resolved++;
		}
	}
	if (resolved != input->property_count) {
		ret = -EINVAL;
		goto release_values;
	}
	request->values = drm_atomic_request_create(dev, entries, resolved);
	if (IS_ERR(request->values)) {
		ret = PTR_ERR(request->values);
		request->values = NULL;
	} else {
		ret = 0;
	}
release_values:
	for (i = 0; i < resolved; i++)
		drm_atomic_release_user_value(&entries[i]);
	kvfree(entries);
	if (!ret)
		return request;
fail:
	drm_atomic_free_user_request(request);
	return ERR_PTR(ret);
}
EXPORT_SYMBOL_IF_KUNIT(drm_atomic_resolve_user_request);

const struct drm_atomic_request *
drm_atomic_user_request_values(const struct drm_atomic_user_request *request)
{
	return request->values;
}
EXPORT_SYMBOL_IF_KUNIT(drm_atomic_user_request_values);

unsigned int drm_atomic_user_request_target_count(const struct drm_atomic_user_request *request)
{
	return request->target_count;
}
EXPORT_SYMBOL_IF_KUNIT(drm_atomic_user_request_target_count);

struct drm_mode_object *
drm_atomic_user_request_target(const struct drm_atomic_user_request *request, unsigned int index)
{
	return index < request->target_count ? request->targets[index] : NULL;
}
EXPORT_SYMBOL_IF_KUNIT(drm_atomic_user_request_target);
