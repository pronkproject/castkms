/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_ATOMIC_PREPARE_TICKET_H__
#define __DRM_ATOMIC_PREPARE_TICKET_H__

#include <linux/wait.h>

struct drm_prepare_retirement_set;
struct drm_prepare_retirement_guard;
struct drm_prepare_ticket;
struct drm_prepare_attempt;

/*
 * Internal ticket ownership, independent of files and display scope validation.
 * Creation retains the borrowed set. Reference release is not cancellation;
 * a transport owner must cancel explicitly when its authority ends. All calls
 * require live references and may sleep. No operation grants pixel access.
 */
struct drm_prepare_ticket *
drm_prepare_ticket_create(struct drm_prepare_retirement_set *set);
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
void drm_prepare_attempt_destroy(struct drm_prepare_attempt *attempt);

/*
 * Serialize installation with cancellation and single consumption. The caller
 * holds the display locks that stabilize its complete scope. install validates
 * that scope and current authority, returning a negative error BEFORE any state
 * change, or installs the complete update and returns zero. It must not wait,
 * allocate, acquire display locks or reenter ticket/preparation operations.
 * Lock order is display locks, then the private ticket mutex.
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

#endif
