/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_ATOMIC_PREPARE_OWNER_H__
#define __DRM_ATOMIC_PREPARE_OWNER_H__

struct drm_prepare_owner;

/*
 * One continuous issuer lifetime, independent of a DRM file or master reference.
 * The policy owner revokes it when authority ends and creates a fresh object if
 * authority returns. Referencing the object does not extend authority. All calls
 * may sleep. max_tickets bounds registered tickets, including retained terminal
 * tickets; callers must close those tickets to recover capacity.
 */
struct drm_prepare_owner *drm_prepare_owner_create(unsigned int max_tickets);
struct drm_prepare_owner *drm_prepare_owner_get(struct drm_prepare_owner *owner);
void drm_prepare_owner_put(struct drm_prepare_owner *owner);

/*
 * Permanently cancel unaccepted tickets. Returns after concurrent acceptance
 * has finished or been excluded. Accepted reader guards remain valid. The caller
 * must not hold locks needed by an installation callback. Policy code must revoke
 * explicitly before dropping its reference; final reference release is not an
 * authority notification. Revocation is idempotent.
 */
void drm_prepare_owner_revoke(struct drm_prepare_owner *owner);

#endif
