/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_CONSTRAINTS_INTERNAL_H__
#define __DRM_CONSTRAINTS_INTERNAL_H__

struct drm_constraints_catalog;
struct drm_constraints_entry;

struct drm_constraints_output {
	struct drm_constraints_catalog *catalog;
	const struct drm_constraints_output_ops *ops;
};

/*
 * Atomic shutdown only: the caller proves that every output plane is detached,
 * the CRTC is disabled and its binding is unchanged. Serialize against close
 * and acceptance, but permit quiescence after closure. Never selects an entry.
 */
int drm_constraints_catalog_quiesce(struct drm_constraints_catalog *catalog,
				    struct drm_constraints_entry *entry,
				    int (*quiesce)(struct drm_constraints_entry *, void *),
				    void *data);

#endif
