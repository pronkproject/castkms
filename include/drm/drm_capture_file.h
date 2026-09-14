/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef __DRM_CAPTURE_FILE_H__
#define __DRM_CAPTURE_FILE_H__

#include <linux/types.h>
#include <drm/drm_capture_description.h>

struct drm_capture_authority;
struct drm_capture_destination;
struct drm_capture_readiness;
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
 * @get_readiness: optionally transfer one owned notification reference at file
 *                 creation, or return NULL when result notifications are absent;
 *                 called once before publication, never from poll
 * @describe: optionally describe the currently offered final-image configuration;
 *            zero succeeds, a negative errno fails without publishing output
 * @open_stream: optionally open offer under a new nonzero caller-supplied stream
 *               id and positive capacity; failure must not consume the name
 * @close_stream: optionally release one named stream, including after revocation;
 *                closing must not make its name available for reuse
 * @register_destination: optionally retain a checked destination under a new,
 *                        increasing client-local name; failure consumes no name
 * @unregister_destination: optionally remove a destination name, including after
 *                          revocation, without canceling accepted native uses
 *
 * Operations remain immutable until release. The client file does not explicitly
 * revoke its authority before release; provider cleanup must respect other clients.
 * Operation calls are serialized for each client, outside authority admission
 * locks. The callbacks may sleep. Describe and open must check current provider
 * permission. Close must permit cleanup without requiring pixel permission.
 * Open must serialize resource registration with authority revocation; the
 * client mutex does not exclude that separate lifetime transition.
 * Callbacks must not reenter operations on the same client. Final release runs after
 * every callback has returned; mutable client data needs no additional lock
 * when accessed only through these callbacks.
 */
struct drm_capture_client_owner_ops {
	struct module *owner;
	void (*release)(void *data);
	struct drm_capture_readiness *(*get_readiness)(void *data);
	int (*describe)(void *data, struct drm_capture_description *description);
	int (*open_stream)(void *data, u64 id, u64 offer, u32 capacity);
	int (*close_stream)(void *data, u64 id);
	int (*register_destination)(void *data, u64 id,
				    const struct drm_capture_destination *destination);
	int (*unregister_destination)(void *data, u64 id);
};

/*
 * Retain already-authorized client state in an anonymous file, without acquiring
 * revocation ownership. Final fput releases data and an ordinary authority
 * reference; other authority owners remain usable. The final authority put still
 * performs normal authority cleanup. Cloned files share one client lifetime.
 *
 * Success consumes data; failure leaves it owned by the caller. The file pins
 * ops->owner before calling get_readiness. Any notification reference returned
 * by that callback is consumed even when file allocation subsequently fails.
 * The callback must return an owned reference or NULL, never an error pointer.
 *
 * The module pin is independent of the authority. All operations may sleep. The file
 * observes retained result readiness and completed revocation through poll;
 * it exposes neither pixel operations nor DRM primary-node dispatch. HUP is
 * not GPU completion.
 * IN and HUP may coexist while terminal results remain available. A readiness
 * observation neither reserves a result nor guarantees a successful dequeue.
 * No descriptor is installed; a publishing adapter must use close-on-exec.
 */
struct file *drm_capture_client_file_create(
	struct drm_capture_authority *authority,
	const struct drm_capture_client_owner_ops *ops, void *data);

/*
 * Obtain an owned readiness reference from a retained client file without
 * invoking its provider. A different endpoint returns -EINVAL; no notification
 * returns -EOPNOTSUPP. The reference survives file release and revocation, but
 * observes only result availability, not permission or ownership of results.
 * The caller must put the reference when finished, including after removing
 * any waiters registered on its wait queue.
 */
struct drm_capture_readiness *drm_capture_client_get_readiness(struct file *file);

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

/*
 * Open a named stream through a retained client file, without userspace memory
 * access. The provider must validate the offered configuration, current
 * permission, increasing unused stream ID and capacity before admission.
 * Success is zero after admission, with no output publication left to fail.
 * Opening requires both open and close callbacks, otherwise -EOPNOTSUPP.
 * Failure must not consume the ID or retain an unreported stream. IDs are
 * client-local, never reused, and not authority. Call outside DRM, authority,
 * provider lifecycle and reservation locks.
 */
int drm_capture_client_open_stream(struct file *file, u64 id, u64 offer, u32 capacity);

/*
 * Close through the provider even after authority revocation. No new pixel
 * access is authorized. The provider owns stream cleanup and must preserve
 * outstanding native completion duties independently of result delivery.
 * Retain file and call outside locks needed by cleanup. No descriptor is closed.
 */
int drm_capture_client_close_stream(struct file *file, u64 id);

/*
 * Register borrowed storage through the client's serialized provider callback.
 * Retain file, every destination buffer and its immutable metadata throughout
 * the call. Shared validation checks shape and export write access; the provider
 * validates complete layout, resource limits and current capture permission.
 * Successful retention requires provider-owned buffer references. Registration
 * admits no pixel write and needs both registration and cleanup callbacks.
 * Failure must leave the nonzero client-local name retryable. Call outside DRM,
 * authority, provider lifecycle and buffer reservation locks.
 */
int drm_capture_client_register_destination(struct file *file, u64 id,
					    const struct drm_capture_destination *destination);

/*
 * Remove a name without requiring current capture permission or revoking the
 * exported allocation. Accepted operations retain their own storage and native
 * completion duties. Names are never reused; no descriptor is closed. Retain
 * file and call outside every lock needed by provider resource cleanup.
 */
int drm_capture_client_unregister_destination(struct file *file, u64 id);

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
