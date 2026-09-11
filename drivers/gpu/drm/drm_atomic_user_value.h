/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_ATOMIC_USER_VALUE_H__
#define __DRM_ATOMIC_USER_VALUE_H__

#include <drm/drm_atomic_request.h>

struct drm_file;

/*
 * The caller owns the target and serializes property attachment. On success,
 * the entry borrows its target and property but owns its resolved value until
 * release. A retained request takes a separate reference to that value.
 * Failure leaves the destination unchanged. File authority must be checked
 * again when applying a retained request; resolution is not authorization to
 * commit. NULL file is reserved for kernel callers with independently checked
 * authority, as with the underlying DRM object lookup functions.
 */
int drm_atomic_resolve_user_value(struct drm_mode_object *object, struct drm_property *property,
				  struct drm_file *file, u64 value,
				  struct drm_atomic_request_entry *entry);
void drm_atomic_release_user_value(struct drm_atomic_request_entry *entry);

#endif
