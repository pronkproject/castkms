/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_CAPTURE_DELIVERY_H__
#define __DRM_CAPTURE_DELIVERY_H__

struct module;

/**
 * struct drm_capture_delivery_ops - detached destination access
 * @owner: Module retaining run and every dependency of its owned payload.
 * @run: Consume the transferred payload, including its destruction, exactly once.
 *
 * The callback may sleep in exporter operations. It must retain its own storage,
 * preserve outstanding destination access independently of client destruction,
 * and publish completion only after access ends. It must not retain a compositor
 * source or acquire a client queue lock while waiting on the destination.
 */
struct drm_capture_delivery_ops {
	struct module *owner;
	void (*run)(void *data);
};

/**
 * drm_capture_delivery_submit - transfer one independently owned delivery
 * @ops: Immutable callbacks retained by owner through completion.
 * @data: Payload consumed by run on successful submission only.
 *
 * Delivery runs on a dedicated unbound queue, not a general-purpose system queue.
 * Admission is globally bounded and returns -EAGAIN at capacity. Providers must
 * separately bound retained image storage. Success transfers payload ownership;
 * failure leaves it with the caller. No client file is retained by dispatch.
 * The module reference is released in native code after run returns, including
 * payload destruction. Submission does not promise bounded exporter latency.
 */
int drm_capture_delivery_submit(const struct drm_capture_delivery_ops *ops, void *data);

#endif
