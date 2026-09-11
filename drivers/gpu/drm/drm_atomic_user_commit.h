/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_ATOMIC_USER_COMMIT_H__
#define __DRM_ATOMIC_USER_COMMIT_H__

#include <linux/types.h>

struct drm_atomic_user_input;
struct drm_device;
struct drm_file;
struct drm_prepare_owner;
struct drm_mode_object;
struct drm_property;

/*
 * Commit copied property assignments using implicit preparation. The caller keeps
 * the device configuration, file and original issuer alive until return and
 * holds no modeset locks. It must capture the issuer before copying faultable
 * userspace input; reacquired authority must not replace that issuer.
 *
 * Resolves resources once under modeset locks, then rebuilds with current file
 * authority on each attempt. Only ALLOW_MODESET and PAGE_FLIP_EVENT are accepted;
 * explicit preparation descriptors and unsupported properties are rejected.
 * The atomic ioctl adapter checks client capability negotiation separately;
 * legacy single-property commands do not require that negotiation.
 * Completion resources are prepared after readiness and released or installed
 * before the attempted state and locks are dropped. No input arrays or resource
 * identifiers are reread after resolving the request.
 */
int drm_atomic_commit_user_request(struct drm_device *dev, struct drm_file *file,
				   struct drm_prepare_owner *owner, u32 flags, u64 user_data,
				   const struct drm_atomic_user_input *input);

/* Retain one legacy property assignment and use the same blocking machinery. */
int drm_atomic_commit_user_property(struct drm_mode_object *object,
				    struct drm_property *property, u64 value,
				    struct drm_file *file);

#endif
