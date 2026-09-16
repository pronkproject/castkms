/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_CONSTRAINTS_DEVICE_H__
#define __DRM_CONSTRAINTS_DEVICE_H__

struct drm_device;
struct drm_constraints_domain;

/*
 * Opt in before creating CRTCs or registering the device. The device owns one
 * identity domain through mode-config cleanup. These helpers do not publish a
 * userspace property or enable another client capability.
 */
int drm_constraints_device_init(struct drm_device *dev, unsigned int limit);
void drm_constraints_device_fini(struct drm_device *dev);
/* Borrowed domain, or NULL; valid while the caller retains the initialized device. */
struct drm_constraints_domain *drm_constraints_device_domain(struct drm_device *dev);

/*
 * Quiesce participating outputs through prepared blocking atomic requests,
 * restore their fixed defaults, then retire nondefault offers. The caller
 * retains initialized topology/resources, revokes departing pixel access and
 * excludes competing updates, publication and replacement owners throughout.
 * Call without master, modeset or source-reader locks held. Submitted native
 * reads retain their backend and must complete before recovery returns.
 *
 * Failure may leave outputs disabled or some defaults restored. Keep new
 * owners excluded and retry only after resolving the failure. No rollback,
 * source-access grant, list reopening or authority gate is provided here.
 */
int drm_constraints_recover(struct drm_device *dev);

#endif
