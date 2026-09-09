/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_ATOMIC_PREPARE_INTERNAL_H__
#define __DRM_ATOMIC_PREPARE_INTERNAL_H__

struct drm_prepare_source;
struct drm_prepare_admission_hold;

/* Sources are distinct, live and stable for the call. Output is private until success. */
int drm_prepare_hold_sources(struct drm_prepare_source * const *sources,
			     struct drm_prepare_admission_hold **holds,
			     unsigned int count);

#endif
