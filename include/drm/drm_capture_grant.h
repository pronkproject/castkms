/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_CAPTURE_GRANT_H__
#define __DRM_CAPTURE_GRANT_H__

#include <linux/types.h>

struct drm_device;
struct drm_file;
struct file;

/**
 * struct drm_capture_target - display objects for final-image capture
 * @crtc_id: CRTC object ID on the issuing device
 * @connector_id: connector object ID on that same device
 *
 * Nonzero IDs describe a target, not proof that the objects exist, belong to
 * the issuing master or currently form an authorized capture route.
 */
struct drm_capture_target {
	u32 crtc_id;
	u32 connector_id;
};

/**
 * struct drm_capture_files - owned capture and control file references
 * @capture: client endpoint, or NULL before construction
 * @control: separate revocation endpoint, or NULL before construction
 *
 * Each non-NULL field owns a reference. Release outside provider policy locks.
 */
struct drm_capture_files {
	struct file *capture;
	struct file *control;
};

/* Consume both references, clearing the fields before running final release. */
void drm_capture_files_put(struct drm_capture_files *files);

/*
 * Dispatch optional provider issuance of a creator-bound final-image grant.
 * The provider authorizes the file, target, explicit issuance flags, rights and
 * creator-close lifetime.
 * This helper checks device association and returned endpoint identity only;
 * it neither impersonates a master nor authorizes framebuffer pixels.
 *
 * Caller retains the registered dev and its open file, holding no locks needed
 * by provider issuance or cleanup. Success transfers a matching pair into result.
 * Failure leaves result untouched and releases any references returned by the
 * provider. No descriptor is reserved or installed. Nonparticipating devices
 * return -EOPNOTSUPP.
 */
int drm_capture_create_file_grant(struct drm_device *dev, struct drm_file *file,
				  const struct drm_capture_target *target,
				  u32 flags,
				  struct drm_capture_files *result);

#endif
