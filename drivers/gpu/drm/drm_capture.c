// SPDX-License-Identifier: GPL-2.0 OR MIT
/* Bounded final-image requests for kernel-controlled capture providers. */

#include <linux/err.h>
#include <linux/export.h>
#include <linux/kref.h>
#include <linux/limits.h>
#include <linux/list.h>
#include <linux/mutex.h>
#include <linux/slab.h>

#include <drm/drm_capture.h>

enum drm_capture_state {
	DRM_CAPTURE_QUEUED,
	DRM_CAPTURE_CLAIMED,
	DRM_CAPTURE_DONE,
};

struct drm_capture_job {
	struct list_head link;
	struct drm_capture *capture;
	u64 id;
	enum drm_capture_state state;
	int error;
	int status;
	void *data;
	bool discarded;
};

struct drm_capture {
	struct kref ref;
	/* Serializes request admission, state transitions and result access. */
	struct mutex lock;
	struct list_head jobs;
	unsigned int capacity;
	unsigned int count;
	size_t frame_size;
	u64 serial;
	bool revoked;
	bool closed;
};

static void drm_capture_free_job(struct drm_capture_job *job)
{
	list_del(&job->link);
	job->capture->count--;
	kvfree(job->data);
	kfree(job);
}

static void drm_capture_release(struct kref *ref)
{
	struct drm_capture *capture = container_of(ref, struct drm_capture, ref);
	struct drm_capture_job *job, *next;

	/* Claimed jobs hold a reference, so none can remain at the final put. */
	list_for_each_entry_safe(job, next, &capture->jobs, link)
		drm_capture_free_job(job);
	mutex_destroy(&capture->lock);
	kfree(capture);
}

struct drm_capture *drm_capture_get(struct drm_capture *capture)
{
	kref_get(&capture->ref);
	return capture;
}
EXPORT_SYMBOL_GPL(drm_capture_get);

void drm_capture_put(struct drm_capture *capture)
{
	kref_put(&capture->ref, drm_capture_release);
}
EXPORT_SYMBOL_GPL(drm_capture_put);

struct drm_capture *drm_capture_create(unsigned int capacity, size_t frame_size)
{
	struct drm_capture *capture;

	if (!capacity || !frame_size || frame_size > SSIZE_MAX)
		return ERR_PTR(-EINVAL);
	capture = kzalloc_obj(*capture);
	if (!capture)
		return ERR_PTR(-ENOMEM);
	kref_init(&capture->ref);
	mutex_init(&capture->lock);
	INIT_LIST_HEAD(&capture->jobs);
	capture->capacity = capacity;
	capture->frame_size = frame_size;
	return capture;
}
EXPORT_SYMBOL_GPL(drm_capture_create);

int drm_capture_queue(struct drm_capture *capture, u64 *id)
{
	struct drm_capture_job *job;
	int ret = 0;

	mutex_lock(&capture->lock);
	if (capture->revoked || capture->closed) {
		ret = -EKEYREVOKED;
		goto out;
	}
	if (capture->count == capture->capacity) {
		ret = -EAGAIN;
		goto out;
	}
	if (capture->serial == U64_MAX) {
		ret = -EOVERFLOW;
		goto out;
	}
	job = kzalloc_obj(*job);
	if (!job) {
		ret = -ENOMEM;
		goto out;
	}
	/* Allocation under the admission lock also bounds concurrent allocators. */
	job->data = kvzalloc(capture->frame_size, GFP_KERNEL);
	if (!job->data) {
		kfree(job);
		ret = -ENOMEM;
		goto out;
	}
	job->capture = capture;
	job->id = ++capture->serial;
	job->state = DRM_CAPTURE_QUEUED;
	job->status = -EINPROGRESS;
	list_add_tail(&job->link, &capture->jobs);
	capture->count++;
	*id = job->id;
out:
	mutex_unlock(&capture->lock);
	return ret;
}
EXPORT_SYMBOL_GPL(drm_capture_queue);

static struct drm_capture_job *drm_capture_find(struct drm_capture *capture, u64 id)
{
	struct drm_capture_job *job;

	list_for_each_entry(job, &capture->jobs, link)
		if (job->id == id && !job->discarded)
			return job;
	return NULL;
}

static void drm_capture_cancel_job(struct drm_capture_job *job, int error)
{
	if (job->state == DRM_CAPTURE_DONE)
		return;
	if (!job->error)
		job->error = error;
	if (job->state == DRM_CAPTURE_QUEUED) {
		job->status = job->error;
		job->state = DRM_CAPTURE_DONE;
	}
}

int drm_capture_cancel(struct drm_capture *capture, u64 id)
{
	struct drm_capture_job *job;
	int ret = 0;

	mutex_lock(&capture->lock);
	job = drm_capture_find(capture, id);
	if (!job)
		ret = -ENOENT;
	else if (job->state == DRM_CAPTURE_DONE)
		ret = -EALREADY;
	else
		drm_capture_cancel_job(job, -ECANCELED);
	mutex_unlock(&capture->lock);
	return ret;
}
EXPORT_SYMBOL_GPL(drm_capture_cancel);

/**
 * drm_capture_discard - abandon a request without waiting for its provider
 * @capture: live stream
 * @id: request to forget
 *
 * The request becomes inaccessible to query, cancel, copy and acknowledge.
 * Queued or completed storage is freed immediately. A claimed job retains
 * storage, its stream reference and queue credit until provider completion.
 * Discard does not end source access or permit early buffer reuse.
 *
 * Return: zero, or -ENOENT if the request is no longer visible.
 */
int drm_capture_discard(struct drm_capture *capture, u64 id)
{
	struct drm_capture_job *job;
	int ret = 0;

	mutex_lock(&capture->lock);
	job = drm_capture_find(capture, id);
	if (!job)
		ret = -ENOENT;
	else if (job->state == DRM_CAPTURE_CLAIMED)
		job->discarded = true;
	else
		drm_capture_free_job(job);
	mutex_unlock(&capture->lock);
	return ret;
}
EXPORT_SYMBOL_GPL(drm_capture_discard);

void drm_capture_revoke(struct drm_capture *capture)
{
	struct drm_capture_job *job;

	mutex_lock(&capture->lock);
	capture->revoked = true;
	list_for_each_entry(job, &capture->jobs, link)
		drm_capture_cancel_job(job, -EKEYREVOKED);
	mutex_unlock(&capture->lock);
}
EXPORT_SYMBOL_GPL(drm_capture_revoke);

struct drm_capture_job *drm_capture_claim(struct drm_capture *capture)
{
	struct drm_capture_job *job, *found = ERR_PTR(-EAGAIN);

	mutex_lock(&capture->lock);
	if (capture->revoked || capture->closed) {
		found = ERR_PTR(-EKEYREVOKED);
		goto out;
	}
	list_for_each_entry(job, &capture->jobs, link) {
		if (job->state != DRM_CAPTURE_QUEUED)
			continue;
		job->state = DRM_CAPTURE_CLAIMED;
		kref_get(&capture->ref);
		found = job;
		break;
	}
out:
	mutex_unlock(&capture->lock);
	return found;
}
EXPORT_SYMBOL_GPL(drm_capture_claim);

void *drm_capture_job_data(struct drm_capture_job *job)
{
	return job->data;
}
EXPORT_SYMBOL_GPL(drm_capture_job_data);

size_t drm_capture_job_size(struct drm_capture_job *job)
{
	return job->capture->frame_size;
}
EXPORT_SYMBOL_GPL(drm_capture_job_size);

void drm_capture_complete(struct drm_capture_job *job, int status)
{
	struct drm_capture *capture = job->capture;

	if (WARN_ON_ONCE(status > 0))
		status = -EIO;
	mutex_lock(&capture->lock);
	job->status = job->error ?: status;
	job->state = DRM_CAPTURE_DONE;
	if (capture->closed || job->discarded)
		drm_capture_free_job(job);
	mutex_unlock(&capture->lock);
	kref_put(&capture->ref, drm_capture_release);
}
EXPORT_SYMBOL_GPL(drm_capture_complete);

/**
 * drm_capture_publish_snapshot - serve one request from a kernel final image
 * @capture: authorized fixed-size stream
 * @pixels: coherent final image, readable for the duration of the call
 * @size: exact image size, including initialized padding
 *
 * The caller must supply only pixels authorized for the stream's recipient.
 * Neither raw planes nor a framebuffer identity establish that permission.
 * The image is copied into independently owned storage before returning, so
 * result delivery never retains the supplied image. The copy occurs outside
 * the admission lock; a racing revocation still changes the request outcome.
 *
 * Return: zero if a claimed copy completed, -EAGAIN without a waiting request,
 * -EKEYREVOKED if admission is closed, or -EINVAL for a mismatched image size.
 * A zero return describes provider completion, not the retained request status.
 */
int drm_capture_publish_snapshot(struct drm_capture *capture,
				 const void *pixels, size_t size)
{
	struct drm_capture_job *job;

	if (size != capture->frame_size)
		return -EINVAL;
	job = drm_capture_claim(capture);
	if (IS_ERR(job))
		return PTR_ERR(job);
	memcpy(drm_capture_job_data(job), pixels, size);
	drm_capture_complete(job, 0);
	return 0;
}
EXPORT_SYMBOL_GPL(drm_capture_publish_snapshot);

int drm_capture_query(struct drm_capture *capture, u64 id,
		      struct drm_capture_result *result)
{
	struct drm_capture_job *job;
	int ret = 0;

	mutex_lock(&capture->lock);
	job = drm_capture_find(capture, id);
	if (!job) {
		ret = -ENOENT;
	} else {
		result->completed = job->state == DRM_CAPTURE_DONE;
		result->status = job->status;
	}
	mutex_unlock(&capture->lock);
	return ret;
}
EXPORT_SYMBOL_GPL(drm_capture_query);

ssize_t drm_capture_copy_result(struct drm_capture *capture, u64 id,
				void *buffer, size_t size)
{
	struct drm_capture_job *job;
	ssize_t ret;

	mutex_lock(&capture->lock);
	job = drm_capture_find(capture, id);
	if (!job) {
		ret = -ENOENT;
	} else if (job->state != DRM_CAPTURE_DONE) {
		ret = -EAGAIN;
	} else if (job->status) {
		ret = job->status;
	} else if (size < capture->frame_size) {
		ret = -ENOSPC;
	} else {
		memcpy(buffer, job->data, capture->frame_size);
		ret = capture->frame_size;
	}
	mutex_unlock(&capture->lock);
	return ret;
}
EXPORT_SYMBOL_GPL(drm_capture_copy_result);

int drm_capture_ack(struct drm_capture *capture, u64 id)
{
	struct drm_capture_job *job;
	int ret = 0;

	mutex_lock(&capture->lock);
	job = drm_capture_find(capture, id);
	if (!job)
		ret = -ENOENT;
	else if (job->state != DRM_CAPTURE_DONE)
		ret = -EBUSY;
	else
		drm_capture_free_job(job);
	mutex_unlock(&capture->lock);
	return ret;
}
EXPORT_SYMBOL_GPL(drm_capture_ack);

void drm_capture_shutdown(struct drm_capture *capture)
{
	struct drm_capture_job *job, *next;

	mutex_lock(&capture->lock);
	capture->closed = true;
	list_for_each_entry_safe(job, next, &capture->jobs, link) {
		drm_capture_cancel_job(job, -ECANCELED);
		if (job->state != DRM_CAPTURE_CLAIMED)
			drm_capture_free_job(job);
	}
	mutex_unlock(&capture->lock);
}
EXPORT_SYMBOL_GPL(drm_capture_shutdown);

void drm_capture_close(struct drm_capture *capture)
{
	drm_capture_shutdown(capture);
	drm_capture_put(capture);
}
EXPORT_SYMBOL_GPL(drm_capture_close);
