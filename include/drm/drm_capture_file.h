/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef __DRM_CAPTURE_FILE_H__
#define __DRM_CAPTURE_FILE_H__

struct drm_capture_authority;
struct file;
struct module;

/**
 * struct drm_capture_control_owner_ops - lifetime retained by a revocation file
 * @owner: module containing the callbacks and owner data's destruction code
 * @release: destroy the transferred data after revocation, outside authority locks
 *
 * The operations must remain valid and immutable until final file release.
 */
struct drm_capture_control_owner_ops {
	struct module *owner;
	void (*release)(void *data);
};

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

/*
 * Additionally retain one provider-owned lifetime until final file release.
 * Success transfers data to the file; failure leaves data owned by the caller
 * and does not invoke release. ops and ops->release must be non-NULL. The file
 * pins ops->owner independently of the authority provider. Release runs after
 * revocation and before the file's authority/module references are dropped.
 * No additional file operations or descriptor installation are provided.
 */
struct file *drm_capture_control_file_create_owned(
	struct drm_capture_authority *authority,
	const struct drm_capture_control_owner_ops *ops, void *data);

#endif
