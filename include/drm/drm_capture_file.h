/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef __DRM_CAPTURE_FILE_H__
#define __DRM_CAPTURE_FILE_H__

struct drm_capture_authority;
struct file;

/*
 * Create an anonymous revocation file for an already-authorized context.
 * Borrows authority and returns an owned file reference or an error pointer.
 * Final fput revokes; get_file/dup shares that lifetime, not a new revoker.
 * Failure leaves the authority unchanged. All operations may sleep.
 *
 * This helper installs no descriptor. An eventual UAPI caller must reserve
 * descriptors with O_CLOEXEC and finish fallible setup before publication.
 * Even an unpublished successfully created file revokes on final fput.
 */
struct file *drm_capture_control_file_create(struct drm_capture_authority *authority);

#endif
