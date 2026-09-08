/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef __DRM_CAPTURE_AUTHORITY_H__
#define __DRM_CAPTURE_AUTHORITY_H__

#include <linux/types.h>

struct drm_capture_authority;
struct module;
struct wait_queue_head;

/**
 * struct drm_capture_authority_ops - provider-owned authority resources
 * @owner: module containing callbacks, or NULL for built-in code
 * @revoke: synchronously stop new resource admission and initiate safe cleanup
 * @release: release provider context after the final authority reference
 *
 * The provider context owns immutable scope, rights and policy references.
 * Revoke runs once, outside the admission mutex. It must not reenter revoke
 * or wait for a client holding the admission guard. Active GPU work may drain
 * afterward, retaining its own references; revoke is not native completion.
 * Release runs after revoke. The ops and context remain valid until release.
 */
struct drm_capture_authority_ops {
	struct module *owner;
	void (*revoke)(void *data);
	void (*release)(void *data);
};

/*
 * Internal lifetime primitive, not a DRM authorization policy or public UAPI.
 * Create takes ownership of provider context only on success. The provider
 * must authorize scope/rights/master/content explicitly before granting access.
 * All operations may sleep and require an owned reference. Final put revokes.
 */
struct drm_capture_authority *
drm_capture_authority_create(const struct drm_capture_authority_ops *ops, void *data);
struct drm_capture_authority *drm_capture_authority_get(struct drm_capture_authority *authority);
void drm_capture_authority_put(struct drm_capture_authority *authority);
void drm_capture_authority_revoke(struct drm_capture_authority *authority);

/*
 * Begin/end guard admission against terminal revoke only. A successful begin
 * holds the mutex until end. Provider policy validation and resource admission
 * belong inside that interval; success alone does not confer pixel permission.
 * Do not call revoke or release the last reference while holding the guard.
 */
int drm_capture_authority_begin(struct drm_capture_authority *authority);
void drm_capture_authority_end(struct drm_capture_authority *authority);
bool drm_capture_authority_revoked(struct drm_capture_authority *authority);
bool drm_capture_authority_cleanup_done(struct drm_capture_authority *authority);
struct wait_queue_head *drm_capture_authority_waitqueue(struct drm_capture_authority *authority);

#endif
