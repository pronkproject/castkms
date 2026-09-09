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
