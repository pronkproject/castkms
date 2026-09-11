/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_ATOMIC_USER_REQUEST_H__
#define __DRM_ATOMIC_USER_REQUEST_H__

#include <linux/types.h>

struct drm_atomic_request;
struct drm_atomic_user_input;
struct drm_atomic_user_request;
struct drm_device;
struct drm_file;
struct drm_mode_object;

/*
 * Resolve a copied ioctl under the caller's modeset locks. The caller keeps
 * the configuration alive until destruction. Values and every target group,
 * including groups without properties, retain their own references. No state
 * is applied, checked or committed. Output pointers and preparation descriptors
 * are not accepted. The caller must revalidate authority before each attempt.
 */
struct drm_atomic_user_request *
drm_atomic_resolve_user_request(struct drm_device *dev, struct drm_file *file,
				const struct drm_atomic_user_input *input);
void drm_atomic_free_user_request(struct drm_atomic_user_request *request);

/* Borrowed, immutable views lasting only as long as their containing request. */
const struct drm_atomic_request *
drm_atomic_user_request_values(const struct drm_atomic_user_request *request);
unsigned int drm_atomic_user_request_target_count(const struct drm_atomic_user_request *request);
struct drm_mode_object *
drm_atomic_user_request_target(const struct drm_atomic_user_request *request, unsigned int index);

/*
 * Recheck file authority under modeset locks without resolving any resource
 * identifier again. The caller separately preserves its issuer through final
 * acceptance; a successful check does not exclude later master/lease changes.
 * Applying the request checks property attachment. The caller must then run
 * the full driver check before committing the update.
 */
int drm_atomic_validate_user_request(const struct drm_atomic_user_request *request,
				     struct drm_file *file);

#endif
