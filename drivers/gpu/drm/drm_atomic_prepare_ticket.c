// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/err.h>
#include <linux/export.h>
#include <linux/kref.h>
#include <linux/mutex.h>
#include <linux/slab.h>
#include <drm/drm_atomic_prepare.h>
#include <drm/drm_atomic_prepare_ticket.h>

#include "drm_atomic_prepare_internal.h"

struct drm_prepare_ticket {
	struct kref ref;
	struct mutex lock;
	struct drm_prepare_domain *domain;
	struct drm_prepare_retirement_set *set;
	struct drm_prepare_attempt *active;
	bool consumed;
};

struct drm_prepare_attempt {
	struct drm_prepare_ticket *ticket;
	struct drm_prepare_retirement_guard *guard;
};

struct drm_prepare_ticket *
drm_prepare_ticket_create(struct drm_prepare_retirement_set *set)
{
	struct drm_prepare_ticket *ticket = kzalloc_obj(*ticket);
	struct drm_prepare_domain *domain;

	if (!ticket)
		return ERR_PTR(-ENOMEM);
	domain = drm_prepare_retirement_set_domain(set);
	ticket->domain = domain ? drm_prepare_domain_get(domain) : drm_prepare_domain_create();
	if (IS_ERR(ticket->domain)) {
		int error = PTR_ERR(ticket->domain);

		kfree(ticket);
		return ERR_PTR(error);
	}
	kref_init(&ticket->ref);
	mutex_init(&ticket->lock);
	ticket->set = drm_prepare_retirement_set_get(set);
	return ticket;
}
EXPORT_SYMBOL_GPL(drm_prepare_ticket_create);

struct drm_prepare_ticket *drm_prepare_ticket_get(struct drm_prepare_ticket *ticket)
{
	kref_get(&ticket->ref);
	return ticket;
}
EXPORT_SYMBOL_GPL(drm_prepare_ticket_get);

static void ticket_free(struct kref *ref)
{
	struct drm_prepare_ticket *ticket = container_of(ref, struct drm_prepare_ticket, ref);

	if (ticket->set)
		drm_prepare_retirement_set_put(ticket->set);
	drm_prepare_domain_put(ticket->domain);
	mutex_destroy(&ticket->lock);
	kfree(ticket);
}

void drm_prepare_ticket_put(struct drm_prepare_ticket *ticket)
{
	kref_put(&ticket->ref, ticket_free);
}
EXPORT_SYMBOL_GPL(drm_prepare_ticket_put);

wait_queue_head_t *drm_prepare_ticket_waitqueue(struct drm_prepare_ticket *ticket)
{
	return drm_prepare_domain_waitqueue(ticket->domain);
}
EXPORT_SYMBOL_GPL(drm_prepare_ticket_waitqueue);

void drm_prepare_ticket_cancel(struct drm_prepare_ticket *ticket)
{
	struct drm_prepare_retirement_set *set;

	mutex_lock(&ticket->lock);
	set = ticket->set;
	ticket->set = NULL;
	mutex_unlock(&ticket->lock);
	wake_up_all(drm_prepare_ticket_waitqueue(ticket));
	if (set)
		drm_prepare_retirement_set_put(set);
}
EXPORT_SYMBOL_GPL(drm_prepare_ticket_cancel);

static int ticket_readiness(struct drm_prepare_ticket *ticket,
			    struct drm_prepare_retirement_set *set)
{
	int ready = drm_prepare_retirement_set_ready(set);

	/* Ready is monotonic under the retained set; check ticket lifetime afterward. */
	mutex_lock(&ticket->lock);
	if (ticket->consumed)
		ready = -EALREADY;
	else if (!ticket->set)
		ready = -ECANCELED;
	mutex_unlock(&ticket->lock);
	return ready;
}

int drm_prepare_ticket_ready(struct drm_prepare_ticket *ticket)
{
	struct drm_prepare_retirement_set *set;
	int ready;

	mutex_lock(&ticket->lock);
	if (ticket->consumed || !ticket->set) {
		ready = ticket->consumed ? -EALREADY : -ECANCELED;
		mutex_unlock(&ticket->lock);
		return ready;
	}
	set = drm_prepare_retirement_set_get(ticket->set);
	mutex_unlock(&ticket->lock);
	ready = ticket_readiness(ticket, set);
	drm_prepare_retirement_set_put(set);
	return ready;
}
EXPORT_SYMBOL_GPL(drm_prepare_ticket_ready);

enum drm_prepare_ticket_status
drm_prepare_ticket_status(struct drm_prepare_ticket *ticket)
{
	switch (drm_prepare_ticket_ready(ticket)) {
	case 0:
		return DRM_PREPARE_TICKET_READY;
	case -EAGAIN:
		return DRM_PREPARE_TICKET_PENDING;
	case -EALREADY:
		return DRM_PREPARE_TICKET_CONSUMED;
	case -ECANCELED:
		return DRM_PREPARE_TICKET_CANCELED;
	default:
		return DRM_PREPARE_TICKET_FAILED;
	}
}
EXPORT_SYMBOL_GPL(drm_prepare_ticket_status);

struct ticket_wait {
	struct drm_prepare_ticket *ticket;
	struct drm_prepare_retirement_set *set;
};

static int waiting_ticket_ready(void *data)
{
	struct ticket_wait *wait = data;

	return ticket_readiness(wait->ticket, wait->set);
}

int drm_prepare_ticket_wait(struct drm_prepare_ticket *ticket)
{
	struct drm_prepare_retirement_set *set;
	wait_queue_head_t *queue;
	struct ticket_wait wait;
	int ret;

	mutex_lock(&ticket->lock);
	if (ticket->consumed || !ticket->set) {
		ret = ticket->consumed ? -EALREADY : -ECANCELED;
		mutex_unlock(&ticket->lock);
		return ret;
	}
	set = drm_prepare_retirement_set_get(ticket->set);
	mutex_unlock(&ticket->lock);
	queue = drm_prepare_retirement_set_waitqueue(set);
	wait = (struct ticket_wait) { .ticket = ticket, .set = set };
	ret = drm_prepare_wait_until_ready(queue, waiting_ticket_ready, &wait);
	drm_prepare_retirement_set_put(set);
	return ret;
}
EXPORT_SYMBOL_GPL(drm_prepare_ticket_wait);

static int ticket_available(struct drm_prepare_ticket *ticket)
{
	lockdep_assert_held(&ticket->lock);
	if (ticket->consumed)
		return -EALREADY;
	if (!ticket->set)
		return -ECANCELED;
	if (ticket->active)
		return -EBUSY;
	return 0;
}

struct drm_prepare_attempt *drm_prepare_ticket_reserve(struct drm_prepare_ticket *ticket)
{
	struct drm_prepare_retirement_set *set;
	struct drm_prepare_attempt *attempt;
	int error;

	mutex_lock(&ticket->lock);
	error = ticket_available(ticket);
	if (error) {
		mutex_unlock(&ticket->lock);
		return ERR_PTR(error);
	}
	set = drm_prepare_retirement_set_get(ticket->set);
	mutex_unlock(&ticket->lock);

	attempt = kmalloc_obj(*attempt);
	if (!attempt) {
		drm_prepare_retirement_set_put(set);
		return ERR_PTR(-ENOMEM);
	}
	attempt->guard = drm_prepare_retirement_guard_create(set);
	drm_prepare_retirement_set_put(set);
	if (IS_ERR(attempt->guard)) {
		error = PTR_ERR(attempt->guard);
		goto free_attempt;
	}

	mutex_lock(&ticket->lock);
	error = ticket_available(ticket);
	if (!error) {
		attempt->ticket = drm_prepare_ticket_get(ticket);
		ticket->active = attempt;
	}
	mutex_unlock(&ticket->lock);
	if (!error)
		return attempt;
	drm_prepare_retirement_guard_destroy(attempt->guard);
free_attempt:
	kfree(attempt);
	return ERR_PTR(error);
}
EXPORT_SYMBOL_GPL(drm_prepare_ticket_reserve);

void drm_prepare_attempt_destroy(struct drm_prepare_attempt *attempt)
{
	struct drm_prepare_ticket *ticket = attempt->ticket;

	mutex_lock(&ticket->lock);
	if (ticket->active == attempt)
		ticket->active = NULL;
	mutex_unlock(&ticket->lock);
	if (attempt->guard)
		drm_prepare_retirement_guard_destroy(attempt->guard);
	drm_prepare_ticket_put(ticket);
	kfree(attempt);
}
EXPORT_SYMBOL_GPL(drm_prepare_attempt_destroy);

int drm_prepare_attempt_commit(struct drm_prepare_attempt *attempt,
			       int (*install)(void *data), void *data,
			       struct drm_prepare_retirement_guard **guard)
{
	struct drm_prepare_ticket *ticket = attempt->ticket;
	struct drm_prepare_retirement_set *set = NULL;
	int error;

	mutex_lock(&ticket->lock);
	if (ticket->consumed) {
		error = -EALREADY;
		goto unlock;
	}
	if (!ticket->set) {
		error = -ECANCELED;
		goto unlock;
	}
	error = install(data);
	if (error)
		goto unlock;
	*guard = attempt->guard;
	attempt->guard = NULL;
	ticket->active = NULL;
	ticket->consumed = true;
	set = ticket->set;
	ticket->set = NULL;
unlock:
	mutex_unlock(&ticket->lock);
	if (set) {
		wake_up_all(drm_prepare_ticket_waitqueue(ticket));
		drm_prepare_retirement_set_put(set);
	}
	return error;
}
EXPORT_SYMBOL_GPL(drm_prepare_attempt_commit);
