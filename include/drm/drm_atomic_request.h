/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_ATOMIC_REQUEST_H__
#define __DRM_ATOMIC_REQUEST_H__

#include <linux/types.h>

struct dma_fence;
struct drm_atomic_request;
struct drm_device;
struct drm_framebuffer;
struct drm_mode_object;
struct drm_property;
struct drm_property_blob;

enum drm_atomic_request_value_type {
	DRM_ATOMIC_REQUEST_SCALAR,
};

/**
 * struct drm_atomic_request_entry - one resolved property assignment
 * @object: target controller, plane, connector or color operation
 * @property: property attached to the target
 * @type: interpretation of the resolved value
 * @scalar: copied numeric value, not a resource identifier
 *
 * The caller keeps the entries unchanged and owns all supplied references until
 * request creation returns.
 * A successful request takes its own references and copies the entries in
 * order, including duplicate assignments. Creation validates representation
 * and device membership, not permission or driver acceptance of an update.
 */
struct drm_atomic_request_entry {
	struct drm_mode_object *object;
	struct drm_property *property;
	enum drm_atomic_request_value_type type;
	union {
		u64 scalar;
	};
};

/*
 * Immutable kernel request storage, independent of files and descriptor lookup.
 * The caller serializes creation against property attachment changes and keeps
 * the device's modeset configuration alive through request destruction. Fixed
 * modeset objects and properties do not acquire a lifetime through object refs.
 * Consumers must revalidate authority and attachment on each attempted update.
 * Driver-private numeric properties must not hide unretained resource handles.
 * Preparation descriptors and output-signaling pointers are not supported.
 */
struct drm_atomic_request *
drm_atomic_request_create(struct drm_device *dev,
			  const struct drm_atomic_request_entry *entries,
			  unsigned int count);
void drm_atomic_request_destroy(struct drm_atomic_request *request);
unsigned int drm_atomic_request_count(const struct drm_atomic_request *request);
const struct drm_atomic_request_entry *
drm_atomic_request_entry(const struct drm_atomic_request *request, unsigned int index);

#endif
