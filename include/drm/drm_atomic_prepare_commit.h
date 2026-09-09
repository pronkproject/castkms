/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_ATOMIC_PREPARE_COMMIT_H__
#define __DRM_ATOMIC_PREPARE_COMMIT_H__

struct drm_atomic_commit;
struct drm_prepare_ticket;

/*
 * Reserve preparation for an exclusively owned, unaccepted atomic transaction.
 * The caller validates complete source scope and authority and stabilizes them
 * through installation, as required by drm_atomic_helper_swap_state_prepared().
 * Reservation does not authenticate scope or freeze the transaction contents.
 * Failure leaves the transaction unchanged. Async plane updates are unsupported.
 * Clearing the transaction abandons its reservation without canceling the ticket.
 */
int drm_atomic_commit_prepare(struct drm_atomic_commit *state,
			     struct drm_prepare_ticket *ticket);

/*
 * Atomic core/helper integration. Installation has the callback contract of
 * drm_prepare_attempt_commit(). An accepted transaction retains admission through
 * object cleanup. Waits concern only already submitted native readers; completion
 * errors end access but do not certify valid pixels. Clear is called only by
 * atomic core after waiting for readers and destroying the transaction's object
 * states. Drivers with their own commit tails must wait before signaling display
 * retirement or releasing old source use. All operations may sleep.
 */
int drm_atomic_commit_preparation_install(struct drm_atomic_commit *state,
					 int (*install)(void *data));
void drm_atomic_commit_wait_for_readers(struct drm_atomic_commit *state);
void drm_atomic_commit_preparation_clear(struct drm_atomic_commit *state);

#endif
