/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_ATOMIC_PREPARE_COMMIT_H__
#define __DRM_ATOMIC_PREPARE_COMMIT_H__

struct drm_atomic_commit;
struct drm_prepare_ticket;
struct drm_prepare_owner;
struct drm_prepare_output_generation;

/*
 * Observe every output retired by the final transaction, including outputs added
 * by atomic checking. Return the entry count or a negative error. The callback
 * must not write more than capacity entries. Display locks stabilize the sources
 * from observation through installation; entries borrow those source references.
 * Observation runs at installation, not reservation, and must not change state.
 */
typedef int (*drm_atomic_prepare_observe_fn)(struct drm_atomic_commit *state,
					   struct drm_prepare_output_generation *entries,
					   unsigned int capacity);

/*
 * Reserve preparation for an exclusively owned, unaccepted atomic transaction.
 * The caller stabilizes all retiring output generations and current authority
 * through installation, as required by drm_atomic_helper_swap_state_prepared().
 * Reservation does not validate output generations or freeze transaction contents.
 * Failure leaves the transaction unchanged. Async plane updates are unsupported.
 * Clearing the transaction abandons its reservation without canceling the ticket.
 *
 * Every ticket requires a provider's observation of the final output generations.
 * The caller separately authenticates the device and continuous caller authority
 * and holds the required display locks through installation. The callback has
 * static lifetime. Clearing the transaction drops it along with the reservation.
 */
int drm_atomic_commit_prepare(struct drm_atomic_commit *state,
				    struct drm_prepare_ticket *ticket,
				    drm_atomic_prepare_observe_fn observe);

/*
 * Reserve a ticket issued by the given owner, with the same transaction contract
 * as drm_atomic_commit_prepare(). NULL owners and tickets from another issuer
 * are rejected. The caller obtains the current issuer before acquiring display
 * locks; retaining that reference does not preserve authority. Revocation after
 * reservation still prevents installation. The attempt retains the ticket and
 * its issuer independently of the caller's references.
 */
int drm_atomic_commit_prepare_owned(struct drm_atomic_commit *state,
				   struct drm_prepare_ticket *ticket,
				   struct drm_prepare_owner *owner,
				   drm_atomic_prepare_observe_fn observe);

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
