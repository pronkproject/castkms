/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_ATOMIC_USER_SIGNALING_H__
#define __DRM_ATOMIC_USER_SIGNALING_H__

#include <linux/types.h>

struct drm_atomic_commit;
struct drm_atomic_user_signaling;
struct drm_file;
struct drm_crtc;

/*
 * Prepare completion resources for one attempted ioctl, under its modeset
 * locks. The caller initializes *signaling to NULL and completes it exactly
 * once, even after a partial failure. TEST_ONLY allocates nothing. Output
 * pointers are taken from the attempted state. No display state is checked,
 * changed or installed; inactive outputs cannot request completion events.
 * Call completion before clearing the attempt or dropping its locks. Accepted
 * events then belong to the display commit and installed fences to their files.
 */
int drm_atomic_prepare_user_signaling(struct drm_atomic_commit *state,
				      struct drm_file *file, u32 flags, u64 user_data,
				      struct drm_atomic_user_signaling **signaling);
/* Preserve event recipients selected before driver checking adds more state. */
int drm_atomic_prepare_user_signaling_for_crtcs(struct drm_atomic_commit *state,
						struct drm_file *file, u32 flags, u64 user_data,
						u32 event_crtcs,
						struct drm_atomic_user_signaling **signaling);
void drm_atomic_complete_user_signaling(struct drm_atomic_commit *state,
					struct drm_atomic_user_signaling *signaling,
					bool accepted);

/*
 * Prepare one legacy flip event for an included controller, ignoring additional
 * controllers brought into the state by driver checks. No output-fence pointer
 * is accepted on the target. The same exactly-once completion rule applies.
 */
int drm_atomic_prepare_user_flip_event(struct drm_atomic_commit *state,
				       struct drm_crtc *crtc, struct drm_file *file,
				       u64 user_data, struct drm_atomic_user_signaling **result);

#endif
