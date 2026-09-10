/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_ATOMIC_PREPARE_AUTH_H__
#define __DRM_ATOMIC_PREPARE_AUTH_H__

struct drm_file;
struct drm_master;
struct drm_prepare_owner;

/*
 * Return a retained preparation issuer for a current modesetting master file.
 * Duplicated descriptors share identity. Other files associated with the same
 * master do not acquire its identity. Master loss, lease revocation and final
 * file close cancel existing tickets; later master acquisition gets a fresh
 * identity. The caller must separately validate every selected display object.
 *
 * Takes master_mutex then mode_config.idr_mutex; do not hold display locks.
 * The returned reference preserves identity, not authority. May sleep.
 */
struct drm_prepare_owner *drm_file_prepare_owner(struct drm_file *file);

/* DRM lifecycle hooks, called before releasing display state or authority. */
void drm_file_cancel_preparation(struct drm_file *file);
void drm_master_cancel_preparation(struct drm_master *master);

/* Cancel one master under mode_config.idr_mutex, including from lease teardown. */
void drm_master_cancel_preparation_locked(struct drm_master *master);

#endif
