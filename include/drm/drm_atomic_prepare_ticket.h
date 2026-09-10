/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_ATOMIC_PREPARE_TICKET_H__
#define __DRM_ATOMIC_PREPARE_TICKET_H__

#include <linux/wait.h>

struct drm_prepare_retirement_set;
struct drm_prepare_retirement_guard;
struct drm_prepare_ticket;
struct drm_prepare_attempt;
struct drm_prepare_scope_entry;
struct drm_prepare_owner;

enum drm_prepare_ticket_status {
	DRM_PREPARE_TICKET_PENDING,
	DRM_PREPARE_TICKET_READY,
	DRM_PREPARE_TICKET_CONSUMED,
	DRM_PREPARE_TICKET_CANCELED,
	DRM_PREPARE_TICKET_FAILED,
};

/*
 * Observe submission preparation, not native reader completion. READY includes
 * a ticket reserved by another attempt and does not promise successful reserve.
 * Cancellation or consumption can follow any observation. FAILED means source
 * accounting cannot establish read closure; it does not describe a GPU error.
 * No status grants authority or validates a transaction's display scope.
 */
enum drm_prepare_ticket_status
drm_prepare_ticket_status(struct drm_prepare_ticket *ticket);

/*
 * Internal ticket ownership, independent of files and display scope validation.
 * Creation retains the borrowed set. Reference release is not cancellation;
 * a transport owner must cancel explicitly when its authority ends. All calls
 * require live references and may sleep. No operation grants pixel access.
 */
struct drm_prepare_ticket *
drm_prepare_ticket_create(struct drm_prepare_retirement_set *set);

/*
 * Capture a scope and derive the ticket's admission holds from that exact scope.
 * Inputs follow drm_prepare_scope_create(); the caller stabilizes them through
 * construction. Failure releases every acquired reference and hold. The ticket
 * owns the captured scope for its lifetime and cannot accept an unscoped commit.
 * This binds source generations, not device or modesetting authority.
 */
struct drm_prepare_ticket *
drm_prepare_ticket_create_scoped(const struct drm_prepare_scope_entry *entries,
				unsigned int count);

/*
 * Bind a scoped ticket to one continuous issuer lifetime. The ticket retains
 * owner identity without retaining a DRM file. The issuer must revoke that owner
 * on authority loss. Revocation cancels unaccepted tickets and prevents future
 * acceptance through outstanding reservations. Creation may fail with -ENOSPC
 * at the owner's ticket limit, or -ECANCELED after revocation.
 */
struct drm_prepare_ticket *
drm_prepare_ticket_create_owned(struct drm_prepare_owner *owner,
				const struct drm_prepare_scope_entry *entries,
				unsigned int count);
struct drm_prepare_ticket *drm_prepare_ticket_get(struct drm_prepare_ticket *ticket);
void drm_prepare_ticket_put(struct drm_prepare_ticket *ticket);
void drm_prepare_ticket_cancel(struct drm_prepare_ticket *ticket);

/*
 * Interruptibly observe readiness or terminal ticket state. Cancellation wakes
 * pending waits with -ECANCELED; consumption returns -EALREADY. Success does not
 * reserve an attempt, exclude another waiter, or wait for submitted GPU readers.
 * Reservation must recheck ticket state. Do not hold locks needed by claim owners
 * or by cancellation. A wait temporarily retains admission independently of the
 * ticket until it returns. The caller retains its live ticket reference.
 */
int drm_prepare_ticket_wait(struct drm_prepare_ticket *ticket);

/*
 * Nonblocking readiness observation: zero, -EAGAIN, or terminal ticket/source
 * error. The queue is borrowed for the ticket reference's lifetime, including
 * after cancellation or consumption releases its source holds. Register before
 * querying to avoid missed notifications; every wake requires another query.
 * Retaining the queue's ticket keeps notification storage, not source admission.
 * Observers must unregister before releasing their final ticket reference.
 */
int drm_prepare_ticket_ready(struct drm_prepare_ticket *ticket);
wait_queue_head_t *drm_prepare_ticket_waitqueue(struct drm_prepare_ticket *ticket);

/*
 * At most one attempt reserves a ticket. Readiness and native completion are
 * collected before publication; pending claims return -EAGAIN. An attempt owns
 * admission independently of cancellation and retains its ticket. Destroying
 * an unaccepted attempt permits retry unless the ticket was canceled.
 * Attempt operations require exclusive caller ownership.
 */
struct drm_prepare_attempt *drm_prepare_ticket_reserve(struct drm_prepare_ticket *ticket);

/*
 * An owned ticket requires its exact issuer identity for reservation. The
 * unowned reserve entry rejects owned tickets; an unrelated owner returns
 * -EACCES without reserving. The issuer checks device and display authority
 * before supplying its owner. No descriptor number or credential is an identity.
 */
struct drm_prepare_attempt *
drm_prepare_ticket_reserve_owned(struct drm_prepare_ticket *ticket,
				 struct drm_prepare_owner *owner);
void drm_prepare_attempt_destroy(struct drm_prepare_attempt *attempt);

/*
 * Serialize installation with cancellation and single consumption. The caller
 * holds the display locks that stabilize its complete scope. install validates
 * that scope and current authority, returning a negative error BEFORE any state
 * change, or installs the complete update and returns zero. It must not wait,
 * allocate, acquire display locks or reenter ticket/preparation operations.
 * Lock order is display locks, then the issuer's owner lock (when present),
 * then the private ticket mutex. install must not reenter owner operations.
 *
 * Failure leaves *guard untouched and the attempt available for retry or
 * destruction. Success transfers the preassembled guard to *guard, consumes
 * the ticket, and makes further acceptance return -EALREADY. The attempt must
 * still be destroyed. Cancellation after success cannot revoke the guard.
 * Native completion is not waited for, and source storage remains caller-owned.
 */
int drm_prepare_attempt_commit(struct drm_prepare_attempt *attempt,
			       int (*install)(void *data), void *data,
			       struct drm_prepare_retirement_guard **guard);

/*
 * Accept only a scoped ticket matching the complete observed output cohort.
 * Validation occurs inside the ticket's cancellation/consumption decision,
 * before install. The caller holds display locks stabilizing observed entries
 * and retains their sources through installation. install still validates
 * current authority and obeys drm_prepare_attempt_commit()'s callback contract.
 * Scope mismatch leaves the attempt retryable and *guard unchanged. Unscoped
 * tickets return -EINVAL; the unscoped commit entry rejects scoped tickets too.
 */
int drm_prepare_attempt_commit_scoped(struct drm_prepare_attempt *attempt,
				      const struct drm_prepare_scope_entry *observed,
				      unsigned int count,
				      int (*install)(void *data), void *data,
				      struct drm_prepare_retirement_guard **guard);

#endif
