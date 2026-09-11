/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_ATOMIC_POWER_H__
#define __DRM_ATOMIC_POWER_H__

#include <linux/types.h>

struct drm_atomic_commit;
struct drm_connector;

int __must_check drm_atomic_set_connector_power(struct drm_atomic_commit *state,
					       struct drm_connector *connector, bool on);

#endif
