/* SPDX-License-Identifier: MIT */
#ifndef _UAPI_DRM_PREPARE_H_
#define _UAPI_DRM_PREPARE_H_

#include "drm.h"

/* Experimental assignments for the preparation development interface. */
#define DRM_CAP_ATOMIC_PREPARATION 0x16
#define DRM_CLIENT_CAP_ATOMIC_PREPARATION 8

/**
 * struct drm_mode_prepare_replace - Prepare replacement of current output uses
 * @crtc_ids: Pointer to an array of @count_crtcs u32 CRTC object IDs.
 * @count_crtcs: Number of unique CRTCs, from 1 through 32.
 * @flags: Must be zero.
 * @reserved: Must be zero.
 *
 * Requires DRM_CLIENT_CAP_ATOMIC_PREPARATION on the issuing master file.
 * All CRTCs must be accessible through that file. The request captures their
 * currently accepted generations and holds new read admission. It does not
 * wait for preparation readiness, native GPU completion or presentation.
 *
 * Success returns a CLOEXEC preparation descriptor as the ioctl return value;
 * all structure fields are input. Supply that ticket through PREPARE_FD on
 * every covered CRTC in an ordinary atomic request. PREPARE_FD reads as -1
 * and is not persistent display state. A missing or changed output generation
 * rejects acceptance with ESTALE; preparation must then be requested again.
 * Final file release cancels unused preparation. No pixel access is granted.
 *
 * A blocking atomic request with no PREPARE_FD assignment prepares internally
 * on participating devices. Supplying PREPARE_FD, including -1, selects the
 * explicit protocol instead. Nonblocking requests from a negotiated client
 * require a ticket; an absent required ticket returns EINVAL. A supplied ticket
 * that remains pending returns EAGAIN without accepting display state.
 */
struct drm_mode_prepare_replace {
	__u64 crtc_ids;
	__u32 count_crtcs;
	__u32 flags;
	__u64 reserved[2];
};

#define DRM_IOCTL_MODE_PREPARE_REPLACE DRM_IOW(0xD3, struct drm_mode_prepare_replace)

/* Submission preparation states, not GPU completion or framebuffer release. */
#define DRM_PREPARE_PENDING 0
#define DRM_PREPARE_READY 1
#define DRM_PREPARE_CONSUMED 2
#define DRM_PREPARE_CANCELED 3
#define DRM_PREPARE_FAILED 4

/**
 * struct drm_prepare_query - Observe a preparation ticket
 * @status: One of DRM_PREPARE_PENDING, DRM_PREPARE_READY, DRM_PREPARE_CONSUMED,
 *          DRM_PREPARE_CANCELED or DRM_PREPARE_FAILED.
 * @reserved: Returned as zero.
 *
 * All fields are output. Querying does not reserve or consume a ticket.
 * READY means the admitted source reads have been accounted for, not that
 * their GPU work finished. Another transaction may have reserved the ticket;
 * cancellation or consumption may follow any observation. FAILED means source
 * read closure could not be established, not that a GPU fence completed with
 * an error. CONSUMED, CANCELED and FAILED are terminal observations.
 *
 * The query is valid only on the anonymous preparation file, not on a DRM
 * device file or a sync_file. It confers no pixel access or modesetting rights.
 */
struct drm_prepare_query {
	__u32 status;
	__u32 reserved[3];
};

#define DRM_IOCTL_PREPARE_QUERY DRM_IOR(0x00, struct drm_prepare_query)

#endif
