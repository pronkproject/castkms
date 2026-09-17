/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_CONSTRAINTS_EVENTS_H__
#define __DRM_CONSTRAINTS_EVENTS_H__

#include <linux/kconfig.h>

struct drm_constraints_events;
struct drm_file;

/*
 * Allocate bounded per-output notifications for one live file. Creation samples
 * initial generations but starts paused; clients still query on setup and
 * resume. Start after publishing a successful subscription. The caller
 * serializes all control calls and retains the file until destroy returns.
 * Destroy detaches observers and joins work before the file may be freed. It
 * may sleep and must not hold master, modeset, list or event locks. Already
 * queued events retain their storage independently until read or file teardown.
 */
struct drm_constraints_events *drm_constraints_events_create(struct drm_file *file);
void drm_constraints_events_start(struct drm_constraints_events *events);
/* Pause joins work without discarding queued records or releasing storage.
 * Like destroy, call without master, modeset, list or event locks.
 */
void drm_constraints_events_pause(struct drm_constraints_events *events);
void drm_constraints_events_destroy(struct drm_constraints_events *events);

#if IS_ENABLED(CONFIG_DRM_KUNIT_TEST)
void drm_constraints_events_flush(struct drm_constraints_events *events);
#endif

#endif
