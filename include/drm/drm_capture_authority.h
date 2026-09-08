/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef __DRM_CAPTURE_AUTHORITY_H__
#define __DRM_CAPTURE_AUTHORITY_H__

#include <linux/types.h>

struct drm_capture_authority;
struct drm_capture;
struct drm_capture_job;
struct module;
struct wait_queue_head;

/**
 * struct drm_capture_authority_ops - provider-owned authority resources
 * @owner: module containing callbacks, or NULL for built-in code
 * @revoke: synchronously stop new resource admission and initiate safe cleanup
 * @release: release provider context after the final authority reference
 * @authorize_capture: approve current source access for a registered stream;
 *                     zero allows, negative errno denies, NULL disables claims
 *
 * The provider context owns immutable scope, rights and policy references.
 * Revoke runs once, outside the admission mutex. It must not reenter revoke
 * or wait for a client holding the admission guard. Active GPU work may drain
 * afterward, retaining its own references; revoke is not native completion.
 * Release runs after revoke. The ops and context remain valid until release.
 * Authorize runs under the admission mutex before request claim. It must not
 * reenter authority operations or acquire locks in the opposite order. The
 * caller must stabilize source/policy state across authorization and claim,
 * and retain the approved source independently until actual access completes.
 * Authorization must have no submission side effects: the queue may be empty.
 */
struct drm_capture_authority_ops {
	struct module *owner;
	void (*revoke)(void *data);
	void (*release)(void *data);
	int (*authorize_capture)(void *data, struct drm_capture *stream);
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

/*
 * Register an already-authorized stream while holding a successful begin guard.
 * Success retains a stream reference until removal or authority revocation.
 * Duplicate registration within the authority fails with -EEXIST. Providers
 * must not share one stream between incompatible authorization scopes.
 * This does not authorize pixels or replace policy checks at provider claim.
 */
int drm_capture_authority_add_stream_locked(struct drm_capture_authority *authority,
					    struct drm_capture *stream);

/*
 * Remove a stream and shut it down without revoking the authority. Requires
 * live references to authority and stream; do not hold the admission guard.
 * A false return means no registration was found (possibly owned by an
 * in-progress revoke). Only revoke guarantees completion of all revoke cleanup.
 */
bool drm_capture_authority_remove_stream(struct drm_capture_authority *authority,
					 struct drm_capture *stream);

/*
 * Claim through live authority, stream membership and provider policy checks.
 * Requires live references; do not hold the admission guard. The provider must
 * hold any additional source/policy locks needed across this entire call.
 * Denial leaves queued requests unchanged. Success transfers one job, completed
 * exactly once with drm_capture_complete(), independently of authority lifetime.
 */
struct drm_capture_job *
drm_capture_authority_claim_stream(struct drm_capture_authority *authority,
				   struct drm_capture *stream);

#endif
