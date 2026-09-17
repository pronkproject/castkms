/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_CONSTRAINTS_CLIENT_H__
#define __DRM_CONSTRAINTS_CLIENT_H__

#include <linux/types.h>

struct drm_file;

int drm_constraints_client_cap(struct drm_file *file, u64 value);
/* Called before the DRM event queue and file context are freed. */
void drm_constraints_client_release(struct drm_file *file);

#endif
