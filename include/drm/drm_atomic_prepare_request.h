/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_ATOMIC_PREPARE_REQUEST_H__
#define __DRM_ATOMIC_PREPARE_REQUEST_H__

#include <linux/types.h>

struct drm_device;
struct drm_atomic_commit;
struct drm_prepare_owner;

enum drm_atomic_request_result {
	DRM_ATOMIC_REQUEST_COMMIT = 0,
	DRM_ATOMIC_REQUEST_UNCHANGED = 1,
};

/*
 * Execute a blocking kernel request using a private atomic state and modeset
 * acquire context. The caller holds no modeset locks, or other locks needed by
 * readers completing preparation. It keeps the device, callback and request
 * data alive until return and serializes device teardown separately.
 *
 * build describes the same requested operation on each invocation. It populates
 * the supplied empty state using ordinary atomic getters, returning
 * DRM_ATOMIC_REQUEST_COMMIT or a negative error, including genuine
 * lock-acquisition -EDEADLK. DRM_ATOMIC_REQUEST_UNCHANGED ends the request
 * successfully without checking or installing attempted state.
 * The callback must establish that result under the appropriate locks, even
 * when rebuilding after a wait. It must neither install state nor retain
 * borrowed state pointers across calls. References to
 * requested objects, buffers and fences belong to the request data; rebuilding
 * must not resolve mutable userspace arrays, handle tables or descriptor numbers.
 * The callback revalidates any authority required by its operation on each call.
 *
 * Preparation waits discard the attempted state before dropping display locks.
 * Admission stays held across rebuilding for unchanged output generations.
 * Final checking captures all affected outputs, including driver-added ones.
 * Devices without preparation use ordinary blocking atomic acceptance.
 *
 * Returns zero after blocking commit completion or an unchanged result from
 * build, or a negative error. A signal
 * may interrupt preparation; failed source accounting never permits acceptance.
 * No source export or userspace request adapter is provided by this interface.
 */
int drm_atomic_commit_request(struct drm_device *dev,
			      int (*build)(struct drm_atomic_commit *state, void *data),
			      void *data);

/*
 * Rebuild a request under one continuous, revocable issuer lifetime. The caller
 * retains owner until return and revokes it when authority ends. Each attempt
 * uses an owned ticket; revocation wakes preparation and excludes installation
 * through outstanding reservations. Reacquired authority must not substitute a
 * new issuer into an outstanding request. Requires device preparation support.
 * Modeset lock acquisition is interruptible, as are preparation waits.
 *
 * The builder still validates the selected objects and any authority not covered
 * by issuer revocation. The issuer alone grants no object access. A
 * DRM_ATOMIC_REQUEST_UNCHANGED result makes no installation or authority
 * guarantee. All other input, locking
 * and lifetime rules of drm_atomic_commit_request() apply. Rebuilding overlaps
 * tickets, requiring room for two tickets in the issuer's allocation budget.
 */
int drm_atomic_commit_request_owned(struct drm_device *dev,
				    struct drm_prepare_owner *owner,
				    int (*build)(struct drm_atomic_commit *state, void *data),
				    void *data);

/**
 * struct drm_atomic_request_callbacks - build and signal a prepared request
 * @build: populate fresh state, following drm_atomic_commit_request() rules
 * @prepare_signaling: optional setup of completion metadata after readiness
 * @complete_signaling: cleanup or publish that metadata after acceptance
 *
 * The two signaling callbacks must either both be supplied or both be NULL.
 * They run with the checked state's modeset locks still held. They must not
 * change checked display configuration, acquire modeset locks, wait for readers,
 * install state, or retain borrowed state pointers after return.
 *
 * prepare_signaling runs only after checking and preparation have succeeded.
 * It may attach events and completion fences, returning zero or a negative
 * error. complete_signaling runs exactly once for each invocation, including
 * a failed setup, before state or locks are released. Its accepted argument
 * is true only after the driver's commit has succeeded. It must release
 * incomplete resources on failure and publish successful completion resources
 * on acceptance. An unchanged build, failed check or preparation wait invokes
 * neither signaling callback.
 */
struct drm_atomic_request_callbacks {
	int (*build)(struct drm_atomic_commit *state, void *data);
	int (*prepare_signaling)(struct drm_atomic_commit *state, void *data);
	void (*complete_signaling)(struct drm_atomic_commit *state, bool accepted, void *data);
};

/*
 * Execute a request with per-attempt completion metadata. The caller retains
 * callbacks and data until return. A non-NULL owner has the same revocation and
 * device-support requirements as drm_atomic_commit_request_owned(); NULL uses
 * the unowned kernel request path. All other request lifetime and locking rules
 * above apply. Signaling setup never runs while preparation remains pending.
 */
int drm_atomic_commit_request_with_callbacks(struct drm_device *dev,
					     struct drm_prepare_owner *owner,
					     const struct drm_atomic_request_callbacks *callbacks,
					     void *data);

/*
 * Prepare and submit a request without waiting for native commit completion.
 * Preparation itself may block: attempted state and modeset locks are dropped
 * while waiting for readers, exactly as for the blocking command above.
 * After readiness, the driver receives a nonblocking atomic commit. An earlier
 * pending commit may still cause -EBUSY; the caller decides whether to retry.
 *
 * Success means accepted, not presented or completed. Driver-owned state and
 * completion events outlive the call, but callbacks and request data must not
 * be retained. Signaling cleanup runs under modeset locks after acceptance or
 * failure. The same issuer, validation and input-retention rules apply.
 * This is not the nonblocking atomic ioctl path: it is intended for kernel
 * commands and legacy adapters that need internal preparation before submitting
 * a flip whose eventual completion is reported separately.
 */
int drm_atomic_submit_request_with_callbacks(struct drm_device *dev,
					     struct drm_prepare_owner *owner,
					     const struct drm_atomic_request_callbacks *callbacks,
					     void *data);

#endif
