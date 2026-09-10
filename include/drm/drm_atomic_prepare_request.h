/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_ATOMIC_PREPARE_REQUEST_H__
#define __DRM_ATOMIC_PREPARE_REQUEST_H__

struct drm_device;
struct drm_atomic_commit;

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

#endif
