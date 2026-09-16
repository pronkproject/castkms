/* SPDX-License-Identifier: MIT */
#ifndef _UAPI_DRM_CAPTURE_H_
#define _UAPI_DRM_CAPTURE_H_

#include "drm.h"

/* Experimental assignments for the capture development interface. */
#define DRM_CAP_CAPTURE_GRANT 0x17

/**
 * struct drm_capture_grant_files - separate capture and revocation descriptors
 * @capture_fd: Final-image client descriptor, without raw-plane or KMS access.
 * @control_fd: Revocation-only descriptor, without pixel access.
 *
 * Both descriptors are close-on-exec. Closing the last control-file reference
 * revokes the grant; closing the capture client does not revoke other clients.
 * The creating DRM file's final close also revokes, even if both descriptors
 * remain open. Poll reports POLLHUP when revocation cleanup has completed,
 * not when rendering, presentation or any GPU operation completes.
 */
struct drm_capture_grant_files {
	__s32 capture_fd;
	__s32 control_fd;
};

/**
 * struct drm_mode_create_capture_grant - Issue creator-bound final-image capture
 * @crtc_id: Nonzero CRTC object ID on the issuing device.
 * @connector_id: Nonzero connector object ID on that same device.
 * @files: Pointer to writable struct drm_capture_grant_files output storage.
 * @flags: Must be zero.
 * @reserved: Must be zero.
 *
 * Requires the current master file and provider support, reported by
 * DRM_CAP_CAPTURE_GRANT. The provider validates the exact output and retains
 * policy for later capture. Issuance does not itself read pixels, grant access
 * to raw planes or authorize all future display content. Unsupported devices
 * return EOPNOTSUPP; invalid or inaccessible target IDs return ENOENT.
 *
 * All request fields are input. Success returns zero after copying both output
 * descriptor numbers and installing their files. On failure neither descriptor
 * is installed: output memory may have been partially written and must not be
 * used. The explicit output location allows publication to finish all fallible
 * user-memory access before either file becomes live.
 */
struct drm_mode_create_capture_grant {
	__u32 crtc_id;
	__u32 connector_id;
	__u64 files;
	__u32 flags;
	__u32 reserved[3];
};

#define DRM_IOCTL_MODE_CREATE_CAPTURE_GRANT \
	DRM_IOW(0xD4, struct drm_mode_create_capture_grant)

/**
 * struct drm_capture_describe - Observe an offered final-image configuration
 * @id: Nonzero offer name within this capture client, not capture authority.
 * @width: Visible width in pixels.
 * @height: Visible height in pixels.
 * @refresh_millihz: Accepted display refresh rate in millihertz.
 * @mode_flags: DRM_MODE_FLAG_* values for the accepted display mode.
 * @format: DRM fourcc format of the offered image.
 * @max_requests: Maximum requests per stream, not a reservation of queue credit.
 * @modifier: DRM format modifier of the offered image.
 * @reserved: Returned as zero.
 *
 * All fields are output. Valid only on an anonymous capture-client descriptor,
 * not its revocation descriptor or a primary DRM file. The provider checks
 * current permission. Inactive output may return ENODEV, denied content EACCES,
 * absent description support EOPNOTSUPP, and revoked authority EKEYREVOKED.
 *
 * Repeated queries of an unchanged configuration return the same name. Content
 * updates do not alone change that name; a new mode or route interval does,
 * even at equal dimensions. A replaced name is never reused within the client.
 * The name identifies the latest offered configuration, not a buffer, fence,
 * presentation time or right to open it after permission changes.
 *
 * Querying reserves no image storage and starts no rendering. The image layout
 * does not establish destination strides, offsets or exporter compatibility.
 * On error ignore all output, which may have been partially copied. A failed
 * copy does not consume the offer; retry queries the current configuration.
 */
struct drm_capture_describe {
	__u64 id;
	__u32 width;
	__u32 height;
	__u32 refresh_millihz;
	__u32 mode_flags;
	__u32 format;
	__u32 max_requests;
	__u64 modifier;
	__u64 reserved;
};

#define DRM_IOCTL_CAPTURE_DESCRIBE DRM_IOR(0x00, struct drm_capture_describe)

/**
 * struct drm_capture_create_stream - Open an offered configuration in a client
 * @id: Caller-supplied nonzero stream name, greater than all admitted names.
 * @offer: Name returned by the latest successful description query.
 * @capacity: Nonzero maximum outstanding requests, at most the offered limit.
 * @flags: Must be zero.
 * @reserved: Must be zero.
 *
 * Valid only on a capture-client descriptor. All fields are input. Success
 * returns zero after the provider registers the stream; no descriptor or
 * output is published. Failure retains no new stream and does not consume
 * the name. Duplicate descriptors share one client and its stream namespace.
 *
 * The provider rechecks current permission and the exact offered configuration.
 * A replaced offer or non-increasing name returns ESTALE, unavailable storage
 * EBUSY, excessive capacity E2BIG, and revoked authority EKEYREVOKED. An admitted
 * UINT64_MAX name exhausts further creation with EOVERFLOW, without preventing
 * cleanup. Zero names or capacity return EINVAL.
 *
 * Each stream retains its own configuration and bounded resources. Querying
 * another offer does not replace existing streams. Creation does not queue a
 * frame or authorize future pixels; later requests recheck their permission.
 */
struct drm_capture_create_stream {
	__u64 id;
	__u64 offer;
	__u32 capacity;
	__u32 flags;
	__u64 reserved;
};

/**
 * struct drm_capture_destroy_stream - Release a named stream in a client
 * @id: Nonzero name of a stream admitted on the same capture client.
 * @reserved: Must be zero.
 *
 * All fields are input. Success removes the stream and abandons its pending
 * delivery, without revoking sibling streams or canceling shared rendering.
 * The name remains unavailable for reuse. Missing names return ENOENT; zero
 * returns EINVAL. Cleanup remains available after revocation or a modeset.
 * Final client-file release also releases its remaining streams.
 *
 * Success ends every destination write admitted through this stream before
 * returning. Buffers may still have other users: this is not a compositor-source,
 * presentation or downstream-consumer fence. Closing a descriptor alone does
 * not acknowledge this boundary; detached destination access may survive even
 * final file release. EBUSY leaves cleanup retryable while that access ends.
 */
struct drm_capture_destroy_stream {
	__u64 id;
	__u64 reserved;
};

#define DRM_IOCTL_CAPTURE_CREATE_STREAM DRM_IOW(0x01, struct drm_capture_create_stream)
#define DRM_IOCTL_CAPTURE_DESTROY_STREAM DRM_IOW(0x02, struct drm_capture_destroy_stream)

/**
 * struct drm_capture_register_destination - Retain caller-owned final-image storage
 * @id: Nonzero name greater than every previously admitted destination name.
 * @width: Nonzero visible width in pixels.
 * @height: Nonzero visible height in pixels.
 * @format: DRM fourcc, validated by the provider.
 * @num_planes: Number of active image planes, from one through four.
 * @modifier: Explicit DRM format modifier, not DRM_FORMAT_MOD_INVALID.
 * @fds: DMA-BUF descriptors for active planes; inactive entries must be zero.
 * @strides: Byte strides for active planes; inactive entries must be zero.
 * @offsets: Byte offsets for active planes; inactive entries must be zero.
 * @flags: Must be zero.
 * @reserved: Must be zero.
 *
 * All fields are input and valid only on an anonymous capture-client descriptor.
 * The complete input and buffer references are acquired before provider admission.
 * Repeated descriptor numbers within this request resolve to the same retained
 * allocation. No descriptor number is retained as an object identity or closed.
 * Success retains provider-owned references; failure consumes no name. Closing
 * or reusing an input descriptor afterward does not replace registered storage.
 * Duplicate capture descriptors share the same destination namespace, separate
 * from stream names. UINT64_MAX exhausts further registration with EOVERFLOW.
 *
 * Buffers must have been exported with write access. The provider validates the
 * complete layout and resource limits. Exporter mapping or GPU import may still
 * fail when storage is used. Registration itself copies no pixels and queues no
 * frame. It grants no future capture permission and does not exclude competing
 * users. Callers must arrange destination reuse before writes.
 * Read-only storage returns EACCES, invalid metadata EINVAL, unsupported layouts
 * EOPNOTSUPP, unavailable registration slots EBUSY and revoked grants EKEYREVOKED.
 * Providers may reject known aliases of retained image registrations with
 * EEXIST. Removing a name does not remove alias tracking retained by active work.
 */
struct drm_capture_register_destination {
	__u64 id;
	__u32 width;
	__u32 height;
	__u32 format;
	__u32 num_planes;
	__u64 modifier;
	__s32 fds[4];
	__u32 strides[4];
	__u64 offsets[4];
	__u32 flags;
	__u32 reserved[3];
};

/**
 * struct drm_capture_unregister_destination - Remove one destination name
 * @id: Nonzero destination name admitted on the same capture client.
 * @reserved: Must be zero.
 *
 * Input only. Removal remains available after revocation; an absent name returns
 * ENOENT. Removed names are never reused. Accepted operations retain their own
 * allocation references and completion duties. Success is not a GPU completion
 * fence and does not revoke an exported allocation. Final client release drops
 * its remaining registrations without revoking other clients' capture grants.
 */
struct drm_capture_unregister_destination {
	__u64 id;
	__u64 reserved;
};

#define DRM_IOCTL_CAPTURE_REGISTER_DESTINATION \
	DRM_IOW(0x03, struct drm_capture_register_destination)
#define DRM_IOCTL_CAPTURE_UNREGISTER_DESTINATION \
	DRM_IOW(0x04, struct drm_capture_unregister_destination)

/**
 * struct drm_capture_queue_output - Admit one output to registered storage
 * @stream: Nonzero name of an open stream on this capture client.
 * @use_id: Nonzero request name greater than all names admitted on that stream.
 * @destination: Nonzero destination name registered on the same client.
 * @reuse_fd: Sync-file descriptor for prior destination use, or -1 for none.
 * @flags: Must be zero.
 * @reserved: Must be zero.
 *
 * All fields are input. Success retains the exact destination and optional
 * native reuse fence, consumes the request name and occupies one stream slot.
 * Closing or reusing the input descriptor afterward does not replace that fence.
 * No output descriptor is installed and no fallible publication follows admission.
 * Removing a destination name does not cancel an accepted write.
 *
 * The caller must exclude competing destination users until its terminal result
 * is dequeued or stream destruction succeeds. A signaled reuse fence does not
 * exclude later users. Known current-source aliases may be rejected; acceptance
 * does not prove independent physical backing or reserve storage against KMS.
 * Destination waits retain no compositor source. Exporter access may run after
 * the ioctl returns; successful admission does not establish valid output pixels.
 *
 * Rejection consumes neither name nor capacity. Invalid fields or a descriptor
 * that is not a sync file return EINVAL, absent names ENOENT, exhausted request
 * slots EAGAIN, non-increasing names ESTALE and revoked authority EKEYREVOKED.
 * Admitting UINT64_MAX exhausts further request names with EOVERFLOW. A fence
 * or exporter error after admission is reported through a terminal result.
 */
struct drm_capture_queue_output {
	__u64 stream;
	__u64 use_id;
	__u64 destination;
	__s32 reuse_fd;
	__u32 flags;
	__u64 reserved;
};

/**
 * struct drm_capture_result - Terminal output metadata without storage ownership
 * @use_id: Original request name within the selected stream.
 * @completed_at_ns: CLOCK_MONOTONIC image-production time, zero on failure.
 * @status: Zero for valid output, negative errno for terminal failure.
 * @reserved: Returned as zero.
 *
 * Destination writes and cache maintenance for this request have ended before
 * publication, including on failure. Failed output may contain partial pixels
 * and must not be used as a valid image. The timestamp describes the produced
 * image, not dequeue, presentation or receiver display. Repeated images may
 * share a timestamp; dequeue order need not match timestamp or request order.
 * Completion says nothing about other users of the same allocation.
 */
struct drm_capture_result {
	__u64 use_id;
	__s64 completed_at_ns;
	__s32 status;
	__u32 reserved;
};

/**
 * struct drm_capture_dequeue - Publish and acknowledge one terminal result
 * @stream: Nonzero stream name on this capture client.
 * @result: Pointer to writable struct drm_capture_result output storage.
 * @reserved: Must be zero.
 *
 * The request fields are input only. Success returns zero after publishing one
 * result and releasing its request slot. EAGAIN means no terminal result exists;
 * it does not wait for rendering or destination reuse. A terminal capture error,
 * including EAGAIN, is carried in result.status with a successful ioctl return.
 *
 * EFAULT retains the same result and slot for retry; ignore all partially copied
 * output. Duplicate descriptors share the queue, so another reader may consume
 * the retained result. Poll readiness is an observation, not a reservation.
 * Dequeue remains available after revocation. No file descriptor or source
 * buffer is published, and acknowledgment grants no authority for later frames.
 * Callers must use an ioctl wrapper that returns EAGAIN rather than retrying it
 * internally, then arrange another observation through poll or their scheduler.
 */
struct drm_capture_dequeue {
	__u64 stream;
	__u64 result;
	__u64 reserved;
};

/**
 * struct drm_capture_cancel - Request cancellation without acknowledging reuse
 * @stream: Nonzero stream name on this capture client.
 * @use_id: Nonzero name of an admitted request on that stream.
 * @reserved: Must be zero.
 *
 * Input only. Cleanup remains available after revocation. Success requests
 * cancellation but does not acknowledge a result or permit destination reuse.
 * Already entered exporter access may still be running. Observe terminal
 * completion or successfully destroy the stream before reusing its storage.
 * Missing names return ENOENT; already cancelled or terminal requests return
 * EALREADY. Cancellation does not wait for destination fences, cancel shared
 * rendering, revoke a grant or return the request's accounting slot.
 */
struct drm_capture_cancel {
	__u64 stream;
	__u64 use_id;
	__u64 reserved;
};

#define DRM_IOCTL_CAPTURE_QUEUE_OUTPUT DRM_IOW(0x05, struct drm_capture_queue_output)
#define DRM_IOCTL_CAPTURE_DEQUEUE DRM_IOW(0x06, struct drm_capture_dequeue)
#define DRM_IOCTL_CAPTURE_CANCEL DRM_IOW(0x07, struct drm_capture_cancel)

#endif
