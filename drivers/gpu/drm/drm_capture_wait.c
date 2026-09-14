// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/export.h>
#include <linux/kthread.h>
#include <linux/sched/signal.h>
#include <drm/drm_capture.h>

int drm_capture_wait_result(struct drm_capture *capture, u64 id,
			    struct drm_capture_result *result)
{
	DEFINE_WAIT_FUNC(wait, woken_wake_function);
	wait_queue_head_t *waitqueue = drm_capture_result_waitqueue(capture);
	struct drm_capture_result observed;
	int ret;

	/* Query while running; the wake flag closes the check-to-sleep race. */
	add_wait_queue(waitqueue, &wait);
	for (;;) {
		ret = drm_capture_query(capture, id, &observed);
		if (ret || observed.completed)
			break;
		if (signal_pending(current) ||
		    (tsk_is_kthread(current) && (kthread_should_stop() || kthread_should_park()))) {
			ret = -ERESTARTSYS;
			break;
		}
		wait_woken(&wait, TASK_INTERRUPTIBLE, MAX_SCHEDULE_TIMEOUT);
	}
	remove_wait_queue(waitqueue, &wait);
	if (ret)
		return ret;
	*result = observed;
	return 0;
}
EXPORT_SYMBOL_GPL(drm_capture_wait_result);

static int capture_provider_wake(struct wait_queue_entry *entry,
				unsigned int mode, int sync, void *key)
{
	wait_queue_head_t *changed = entry->private;

	wake_up_all(changed);
	return 0;
}

/**
 * drm_capture_wait_provider - wait for a provider while observing capture termination
 * @capture: retained stream
 * @id: request awaiting provider work
 * @provider_wait: retained queue notified when provider readiness changes
 * @ready: callback returning positive when ready, zero to wait, or negative errno
 * @data: borrowed callback data, retained until return
 *
 * Observe the request and provider after registering both notifications. The
 * callback runs in the caller's task without capture or waitqueue locks held;
 * it may sleep. Notifications during the callback prevent the following sleep.
 * Hold no locks needed by the provider. The callback must not recursively use
 * the same wait helper. No provider claim is made or ended by waiting.
 *
 * A queued request's cancellation, revocation or removal ends the wait even
 * when the provider has not become ready. An already claimed request still
 * needs its actual completion before cancellation becomes a terminal result.
 *
 * Return: zero for provider readiness, the request's terminal error, -EALREADY
 * for an already successful request, -ENOENT for removal, or a callback error.
 * A signal or kernel-thread stop/park interrupts the wait with -ERESTARTSYS.
 * Success is an observation, not permission to bypass subsequent claim checks.
 */
int drm_capture_wait_provider(struct drm_capture *capture, u64 id,
			      wait_queue_head_t *provider_wait,
			      int (*ready)(void *data), void *data)
{
	DECLARE_WAIT_QUEUE_HEAD_ONSTACK(changed);
	DEFINE_WAIT_FUNC(wait, woken_wake_function);
	DEFINE_WAIT_FUNC(capture_wait, capture_provider_wake);
	DEFINE_WAIT_FUNC(provider, capture_provider_wake);
	wait_queue_head_t *result_wait = drm_capture_result_waitqueue(capture);
	struct drm_capture_result result;
	int ret;

	if (!provider_wait || !ready)
		return -EINVAL;
	/* Both producers notify one queue, serializing updates to the wake flag. */
	capture_wait.private = &changed;
	provider.private = &changed;
	add_wait_queue(&changed, &wait);
	add_wait_queue(result_wait, &capture_wait);
	add_wait_queue(provider_wait, &provider);
	for (;;) {
		ret = drm_capture_query(capture, id, &result);
		if (ret)
			break;
		if (result.completed) {
			ret = result.status ?: -EALREADY;
			break;
		}
		ret = ready(data);
		if (ret) {
			if (ret > 0)
				ret = 0;
			break;
		}
		if (signal_pending(current) ||
		    (tsk_is_kthread(current) && (kthread_should_stop() || kthread_should_park()))) {
			ret = -ERESTARTSYS;
			break;
		}
		wait_woken(&wait, TASK_INTERRUPTIBLE, MAX_SCHEDULE_TIMEOUT);
	}
	remove_wait_queue(provider_wait, &provider);
	remove_wait_queue(result_wait, &capture_wait);
	remove_wait_queue(&changed, &wait);
	return ret;
}
EXPORT_SYMBOL_GPL(drm_capture_wait_provider);
