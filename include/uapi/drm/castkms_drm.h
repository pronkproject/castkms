/* SPDX-License-Identifier: MIT */
#ifndef _UAPI_CASTKMS_DRM_H_
#define _UAPI_CASTKMS_DRM_H_

#include "drm.h"

#include <linux/types.h>

#define DRM_CASTKMS_MONITOR_CONTROL_VERSION 1
#define DRM_CASTKMS_MONITOR_MAX_EDID_SIZE (256U * 128U)

/**
 * struct drm_castkms_create_monitor_control - create virtual monitor control
 * @connector_id: DRM object ID of the virtual connector
 * @flags: must be zero
 * @control_fd: returned close-on-exec monitor-control file descriptor
 * @revoke_fd: returned close-on-exec revocation file descriptor
 * @reserved: must be zero
 *
 * The calling DRM file must be the current master and hold the connector.
 * Only one monitor-control file may exist for a connector. Creation replaces
 * the standalone fallback monitor with a disconnected managed monitor. Final
 * close of the control file, or close of the revocation file, restores the
 * fallback monitor. The control capability remains valid across DRM master
 * changes and may be transferred like any other file descriptor. The issuer
 * retains the revocation file.
 */
struct drm_castkms_create_monitor_control {
	__u32 connector_id;
	__u32 flags;
	__s32 control_fd;
	__s32 revoke_fd;
	__u32 reserved;
};

/**
 * struct drm_castkms_monitor_query - query monitor-control capabilities
 * @version: returned DRM_CASTKMS_MONITOR_CONTROL_VERSION
 * @flags: returned capability flags; currently zero
 * @max_edid_size: maximum accepted complete EDID size in bytes
 * @reserved: must be zero
 */
struct drm_castkms_monitor_query {
	__u32 version;
	__u32 flags;
	__u32 max_edid_size;
	__u32 reserved;
};

/**
 * struct drm_castkms_monitor_attach - publish an attached virtual monitor
 * @flags: must be zero
 * @edid_size: complete EDID size, or zero to use fallback modes
 * @edid_ptr: userspace pointer to @edid_size bytes, or zero without an EDID
 *
 * The driver copies and validates EDID data before changing the monitor.
 */
struct drm_castkms_monitor_attach {
	__u32 flags;
	__u32 edid_size;
	__u64 edid_ptr;
};

/**
 * struct drm_castkms_monitor_detach - disconnect the managed monitor
 * @flags: must be zero
 * @reserved: must be zero
 */
struct drm_castkms_monitor_detach {
	__u32 flags;
	__u32 reserved;
};

#define DRM_CASTKMS_CREATE_MONITOR_CONTROL 0x00
#define DRM_CASTKMS_MONITOR_QUERY 0x01
#define DRM_CASTKMS_MONITOR_ATTACH 0x02
#define DRM_CASTKMS_MONITOR_DETACH 0x03

/* This is an enum so that Rust bindgen resolves the ioctl values. */
enum {
	DRM_IOCTL_CASTKMS_CREATE_MONITOR_CONTROL =
		DRM_IOWR(DRM_COMMAND_BASE + DRM_CASTKMS_CREATE_MONITOR_CONTROL,
			 struct drm_castkms_create_monitor_control),
	DRM_IOCTL_CASTKMS_MONITOR_QUERY =
		DRM_IOR(DRM_COMMAND_BASE + DRM_CASTKMS_MONITOR_QUERY,
			struct drm_castkms_monitor_query),
	DRM_IOCTL_CASTKMS_MONITOR_ATTACH =
		DRM_IOW(DRM_COMMAND_BASE + DRM_CASTKMS_MONITOR_ATTACH,
			struct drm_castkms_monitor_attach),
	DRM_IOCTL_CASTKMS_MONITOR_DETACH =
		DRM_IOW(DRM_COMMAND_BASE + DRM_CASTKMS_MONITOR_DETACH,
			struct drm_castkms_monitor_detach),
};

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
