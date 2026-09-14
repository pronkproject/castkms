/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef __DRM_CAPTURE_FILE_INTERNAL_H__
#define __DRM_CAPTURE_FILE_INTERNAL_H__

struct drm_capture_authority;
struct file;

/* Borrow the client's authority while retaining file; NULL for any other file. */
struct drm_capture_authority *drm_capture_client_authority(struct file *file);

#endif
