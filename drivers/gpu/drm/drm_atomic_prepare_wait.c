// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/sched/signal.h>
#include <linux/kthread.h>
#include <linux/wait.h>

#include "drm_atomic_prepare_internal.h"

/* Readiness may take mutexes; the wake flag closes the check-to-sleep race. */
int drm_prepare_wait_until_ready(wait_queue_head_t *queue, int (*ready)(void *data), void *data)
{
	DEFINE_WAIT_FUNC(wait, woken_wake_function);
	int ret;

	if (!queue)
		return ready(data);
	add_wait_queue(queue, &wait);
	for (;;) {
		ret = ready(data);
		if (ret != -EAGAIN)
			break;
		if (signal_pending(current) ||
		    (tsk_is_kthread(current) && (kthread_should_stop() || kthread_should_park()))) {
			ret = -ERESTARTSYS;
			break;
		}
		wait_woken(&wait, TASK_INTERRUPTIBLE, MAX_SCHEDULE_TIMEOUT);
	}
	remove_wait_queue(queue, &wait);
	return ret;
}
