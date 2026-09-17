/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_CONSTRAINTS_EVENTS_H__
#define __DRM_CONSTRAINTS_EVENTS_H__

#include <linux/kconfig.h>

struct drm_constraints_events;
struct drm_file;

/*
 * Allocate bounded per-output notifications for one live file. Creation samples
 * initial generations; clients still query on setup and resume. The caller
 * serializes creation/destruction and retains the file until destroy returns.
 * Destroy detaches observers and joins work before the file may be freed. It
 * may sleep and must not hold master, modeset, list or event locks. Already
 * queued events retain their storage independently until read or file teardown.
 */
struct drm_constraints_events *drm_constraints_events_create(struct drm_file *file);
void drm_constraints_events_destroy(struct drm_constraints_events *events);

#if IS_ENABLED(CONFIG_DRM_KUNIT_TEST)
void drm_constraints_events_flush(struct drm_constraints_events *events);
#endif

#endif
