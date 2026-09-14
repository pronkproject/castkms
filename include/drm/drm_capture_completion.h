/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef __DRM_CAPTURE_COMPLETION_H__
#define __DRM_CAPTURE_COMPLETION_H__

#include <linux/ktime.h>
#include <linux/types.h>

/**
 * struct drm_capture_completion - terminal delivery metadata, without pixel storage
 * @use_id: nonzero request name within its stream
 * @status: zero for a valid delivered image, negative errno for terminal failure
 * @completed_at: monotonic image-production time on success, zero on failure
 *
 * All destination writes for the attempt have ended before publication, even
 * on error. Completion does not describe presentation or downstream use.
 * The timestamp belongs to the produced image, not the later dequeue call.
 * Reusing an image may repeat its timestamp; dequeue order need not be time order.
 */
struct drm_capture_completion {
	u64 use_id;
	int status;
	ktime_t completed_at;
};

/**
 * struct drm_capture_completion_sink - borrowed publication of one terminal result
 * @publish: consume borrowed metadata; zero acknowledges it, negative errno retries
 * @data: context borrowed only during synchronous publication
 *
 * The provider invokes publish at most once and returns its result unchanged.
 * Publication failure must leave the same result and accounting credit queued.
 * No sink, context or metadata borrow may escape the synchronous dequeue call.
 */
struct drm_capture_completion_sink {
	int (*publish)(void *data, const struct drm_capture_completion *completion);
	void *data;
};

#endif
