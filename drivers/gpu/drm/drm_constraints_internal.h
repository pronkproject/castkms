/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_CONSTRAINTS_INTERNAL_H__
#define __DRM_CONSTRAINTS_INTERNAL_H__

struct drm_constraints_output {
	struct drm_constraints_catalog *catalog;
	const struct drm_constraints_output_ops *ops;
};

#endif
