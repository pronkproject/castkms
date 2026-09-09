/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_CAPTURE_H__
#define __DRM_CAPTURE_H__

#include <linux/types.h>
#include <linux/wait.h>

struct drm_capture;
struct drm_capture_job;

/**
 * struct drm_capture_result - retained outcome of a capture request
 * @completed: provider access has ended, or no access was admitted
 * @status: zero for a valid image, negative errno otherwise; -EINPROGRESS
 *          while pending
 */
struct drm_capture_result {
	bool completed;
	int status;
};

/*
 * Kernel-only first cut: one fixed-size final-image stream per authority.
 * The creating provider must already have authorized the source and recipient.
 * The core does not authorize DRM objects or export raw scanout storage.
 * All entry points may sleep. Calls need a live reference; get requires one
 * already held. Shutdown is idempotent and may race calls holding independent
 * references. Put releases only a reference, without shutting down other
 * owners. Close combines shutdown and put, consuming the caller's reference.
 * Provider jobs independently keep the stream alive and may finish after close.
 */
struct drm_capture *drm_capture_create(unsigned int capacity, size_t frame_size);
struct drm_capture *drm_capture_get(struct drm_capture *capture);
void drm_capture_put(struct drm_capture *capture);
void drm_capture_shutdown(struct drm_capture *capture);
void drm_capture_close(struct drm_capture *capture);
void drm_capture_revoke(struct drm_capture *capture);
int drm_capture_queue(struct drm_capture *capture, u64 *id);
int drm_capture_cancel(struct drm_capture *capture, u64 id);
/* Forget demand/result now; active provider storage and credit retire on completion. */
int drm_capture_discard(struct drm_capture *capture, u64 id);
int drm_capture_query(struct drm_capture *capture, u64 id,
		      struct drm_capture_result *result);
/* Retain capture, register before querying, and recheck after every notification. */
wait_queue_head_t *drm_capture_result_waitqueue(struct drm_capture *capture);
/*
 * Wait interruptibly for a retained terminal result or disappearance of the ID.
 * Zero returns a completed result, whose status may still be a producer error.
 * Failure leaves result unchanged. Neither observation consumes the request.
 * Cancel/revoke of a claimed request still needs actual provider completion;
 * waiting does not turn cancellation into permission to release active storage.
 * Do not hold locks needed by the provider. Caller retains capture throughout.
 */
int drm_capture_wait_result(struct drm_capture *capture, u64 id,
			    struct drm_capture_result *result);
int drm_capture_ack(struct drm_capture *capture, u64 id);
ssize_t drm_capture_copy_result(struct drm_capture *capture, u64 id,
				void *buffer, size_t size);

/*
 * Claim chooses the oldest queued request without waiting. The returned job
 * authorizes only the stream's already-approved final image. Exactly one
 * provider owns it until complete consumes it. Revocation does not revoke
 * that ownership or pretend that access has ended. No core lock is held while
 * the provider fills the job. Never retain the data pointer after completion.
 */
struct drm_capture_job *drm_capture_claim(struct drm_capture *capture);
void *drm_capture_job_data(struct drm_capture_job *job);
size_t drm_capture_job_size(struct drm_capture_job *job);
void drm_capture_complete(struct drm_capture_job *job, int status);

int drm_capture_publish_snapshot(struct drm_capture *capture,
				 const void *pixels, size_t size);

#endif
