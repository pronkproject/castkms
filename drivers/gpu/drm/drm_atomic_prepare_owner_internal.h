/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_ATOMIC_PREPARE_OWNER_INTERNAL_H__
#define __DRM_ATOMIC_PREPARE_OWNER_INTERNAL_H__

#include <linux/list.h>

struct drm_prepare_owner;

struct drm_prepare_owner_registration {
	struct list_head link;
	void (*cancel)(void *data);
	void *data;
};

/*
 * Registration retains the owner, not the ticket. The embedded registration and
 * ticket storage must remain live until unregister returns. Revocation accesses
 * tickets under the owner's lock; ticket destruction unregisters before freeing.
 * cancel must not reenter owner operations or destroy its registration storage.
 */
int drm_prepare_owner_register(struct drm_prepare_owner *owner,
			       struct drm_prepare_owner_registration *registration,
			       void (*cancel)(void *data), void *data);
void drm_prepare_owner_unregister(struct drm_prepare_owner *owner,
				  struct drm_prepare_owner_registration *registration);

/* Display locks -> owner lock -> ticket lock. Success requires unlock. */
int drm_prepare_owner_lock_live(struct drm_prepare_owner *owner);
void drm_prepare_owner_unlock(struct drm_prepare_owner *owner);

#endif
