// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/export.h>
#include <linux/sched.h>
#include <drm/drm_capture.h>

int drm_capture_wait_result(struct drm_capture *capture, u64 id,
			    struct drm_capture_result *result)
{
	struct drm_capture_result observed;
	int ret, interrupted;

	interrupted = wait_event_interruptible(*drm_capture_result_waitqueue(capture), ({
		ret = drm_capture_query(capture, id, &observed);
		ret || observed.completed;
	}));
	if (interrupted)
		return interrupted;
	if (ret)
		return ret;
	*result = observed;
	return 0;
}
EXPORT_SYMBOL_GPL(drm_capture_wait_result);
