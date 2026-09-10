/* SPDX-License-Identifier: MIT */
#ifndef _UAPI_DRM_PREPARE_H_
#define _UAPI_DRM_PREPARE_H_

#include "drm.h"

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
