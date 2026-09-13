/* SPDX-License-Identifier: MIT */
#ifndef _UAPI_CASTKMS_DRM_H_
#define _UAPI_CASTKMS_DRM_H_

#include <linux/types.h>

#define DRM_CASTKMS_EXECUTION_VERSION 1
#define DRM_CASTKMS_EXECUTION_HOST_V1 1

/**
 * struct drm_castkms_execution - CASTKMS_EXECUTION connector blob
 * @version: Description layout version, DRM_CASTKMS_EXECUTION_VERSION.
 * @profile: Active renderer profile, DRM_CASTKMS_EXECUTION_HOST_V1.
 * @generation: Nonzero capability generation within the connector lifetime.
 *
 * The read-only blob describes the renderer, not capture permission or completion.
 * All fields use native byte order. Version 1 has exactly 16 bytes. Unknown
 * versions or profiles must not be interpreted as permission to import buffers.
 * Reading the description reserves neither a generation nor an atomic commit.
 *
 * HOST_V1 requires native CastKMS shmem, linear XRGB8888, and one opaque primary
 * covering the whole output at 1:1 sampling. Width and height are bounded by
 * 1920 and 1080. Source allocations are at most 16 MiB; offsets and strides are
 * aligned to four bytes and the allocation includes every complete stride.
 * No crop, scale, rotation, reflection, hardware cursor, overlay or nonidentity
 * color operation is supported. Clients include the cursor in primary rendering.
 * Disabled outputs and blank active outputs require no source allocation.
 * Successful PRIME import does not establish eligibility for that profile.
 */
struct drm_castkms_execution {
	__u32 version;
	__u32 profile;
	__u64 generation;
};

#endif
