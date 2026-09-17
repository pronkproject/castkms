/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_CONSTRAINTS_INTERNAL_H__
#define __DRM_CONSTRAINTS_INTERNAL_H__

#include <linux/atomic.h>

struct drm_constraints_list;
struct drm_constraints_entry;

struct drm_constraints_selection {
	struct drm_constraints_list *list;
	struct drm_constraints_entry *entry;
	bool quiesce;
};

/*
 * Accept one atomic cohort while holding every distinct list. The caller keeps
 * list/entry references alive and stabilizes modeset authority. Lock contention
 * returns EBUSY without invoking install. The callback must check everything
 * before its single irreversible swap, and must not reenter list operations.
 * Quiescing members retain their exact selection even after list closure.
 */
int drm_constraints_lists_accept(const struct drm_constraints_selection *selections,
				 unsigned int count, int (*install)(void *), void *data);

struct drm_constraints_output {
	struct drm_constraints_list *list;
	struct drm_constraints_entry *default_entry;
	const struct drm_constraints_output_ops *ops;
	/* Adds hold the CRTC lock and idr_mutex; removals hold idr_mutex. */
	atomic_t leases;
};

/*
 * Atomic shutdown only: the caller proves that every output plane is detached,
 * the CRTC is disabled and its binding is unchanged. Serialize against close
 * and acceptance, but permit quiescence after closure. Never selects an entry.
 */
int drm_constraints_list_quiesce(struct drm_constraints_list *list,
				 struct drm_constraints_entry *entry,
				 int (*quiesce)(struct drm_constraints_entry *, void *),
				 void *data);

#endif
