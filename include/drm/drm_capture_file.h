/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef __DRM_CAPTURE_FILE_H__
#define __DRM_CAPTURE_FILE_H__

#include <linux/types.h>
#include <drm/drm_capture_description.h>

struct drm_capture_authority;
struct file;
struct module;

/*
 * Check endpoint roles and shared authority without granting permission or
 * creating another revocation owner. Both file references must remain live.
 * A matching revoked pair still matches; this is not an admission check.
 */
bool drm_capture_files_match(struct file *capture, struct file *control);

/**
 * struct drm_capture_client_owner_ops - lifetime retained by a capture client file
 * @owner: module containing callbacks and the transferred data's destruction code
 * @release: destroy transferred client state outside authority locks
 * @describe: optionally describe the currently offered final-image configuration;
 *            zero succeeds, a negative errno fails without publishing output
 *
 * Operations remain immutable until release. The client file does not explicitly
 * revoke its authority before release; provider cleanup must respect other clients.
 * Describe calls are serialized for each client, outside authority admission
 * locks. The callback may sleep and must check current provider permission.
 * It must not reenter operations on the same client. Final release runs after
 * every callback has returned; mutable client data needs no additional lock
 * when accessed only through these callbacks.
 */
struct drm_capture_client_owner_ops {
	struct module *owner;
	void (*release)(void *data);
	int (*describe)(void *data, struct drm_capture_description *description);
};

/*
 * Retain already-authorized client state in an anonymous file, without acquiring
 * revocation ownership. Final fput releases data and an ordinary authority
 * reference; other authority owners remain usable. The final authority put still
 * performs normal authority cleanup. Cloned files share one client lifetime.
 *
 * Success consumes data; failure leaves it owned by the caller. The file pins
 * ops->owner independently of the authority. All operations may sleep. The file
 * currently observes completed revocation through poll only; it exposes neither
 * pixel operations nor DRM primary-node dispatch. HUP is not GPU completion.
 * No descriptor is installed; a publishing adapter must use close-on-exec.
 */
struct file *drm_capture_client_file_create(
	struct drm_capture_authority *authority,
	const struct drm_capture_client_owner_ops *ops, void *data);

/*
 * Describe through an owned client file without accessing userspace memory.
 * Retain file throughout the call. A different endpoint returns -EINVAL, an
 * absent provider operation returns -EOPNOTSUPP, and terminal revocation
 * returns -EKEYREVOKED. Success copies validated metadata to description;
 * failure leaves it unchanged. Call outside DRM and authority locks.
 * Success is an observation, not permission for a later stream or pixel access.
 */
int drm_capture_client_describe(struct file *file,
			       struct drm_capture_description *description);

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
