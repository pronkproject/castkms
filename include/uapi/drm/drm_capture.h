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
	__u32 format;
	__u32 max_requests;
	__u64 modifier;
	__u64 reserved[2];
};

#define DRM_IOCTL_CAPTURE_DESCRIBE DRM_IOR(0x00, struct drm_capture_describe)

#endif
