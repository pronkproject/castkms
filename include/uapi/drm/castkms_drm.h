/* SPDX-License-Identifier: MIT */
#ifndef _UAPI_CASTKMS_DRM_H_
#define _UAPI_CASTKMS_DRM_H_

#include "drm.h"

#include <linux/types.h>

#define DRM_CASTKMS_MONITOR_CONTROL_VERSION 1
#define DRM_CASTKMS_MONITOR_MAX_EDID_SIZE (256U * 128U)
#define DRM_CASTKMS_RENDERER_VERSION 1

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

/**
 * struct drm_castkms_renderer_files - renderer and revocation descriptors
 * @renderer_fd: close-on-exec renderer file descriptor
 * @revoke_fd: close-on-exec revocation file descriptor
 *
 * Final close of the revocation descriptor permanently rejects further
 * operations through every duplicate of the renderer descriptor. Closing a
 * renderer descriptor releases only that reference.
 */
struct drm_castkms_renderer_files {
	__s32 renderer_fd;
	__s32 revoke_fd;
};

/**
 * struct drm_castkms_create_renderer_control - create renderer control
 * @crtc_id: DRM object ID of the controlled CRTC
 * @connector_id: DRM object ID of the controlled connector
 * @files: pointer to writable struct drm_castkms_renderer_files output storage
 * @flags: must be zero
 * @reserved: must be zero
 *
 * The calling DRM file must be the exact current master and hold both display
 * objects. The renderer descriptor grants no modesetting or capture access.
 * Its operations authorize delegated rendering for this output and master
 * interval.
 *
 * All request fields are input. Success returns zero after copying both output
 * descriptor numbers and installing their files. On failure neither descriptor
 * is installed; output memory may have been partially written and must not be
 * used. No fallible operation remains after descriptor installation.
 */
struct drm_castkms_create_renderer_control {
	__u32 crtc_id;
	__u32 connector_id;
	__u64 files;
	__u32 flags;
	__u32 reserved[3];
};

/**
 * struct drm_castkms_renderer_query - query current renderer control
 * @version: returned DRM_CASTKMS_RENDERER_VERSION
 * @flags: returned capability flags; currently zero
 * @profile: current DRM_CASTKMS_EXECUTION_* profile
 * @reserved: must be zero
 * @generation: current nonzero execution generation
 *
 * Query succeeds only while this renderer capability, its issuing master
 * interval, and its exact enabled output remain current. A successful query is
 * an observation; it does not reserve a later takeover or source operation.
 */
struct drm_castkms_renderer_query {
	__u32 version;
	__u32 flags;
	__u32 profile;
	__u32 reserved;
	__u64 generation;
};

#define DRM_CASTKMS_CREATE_MONITOR_CONTROL 0x00
#define DRM_CASTKMS_CREATE_RENDERER_CONTROL 0x01
#define DRM_CASTKMS_MONITOR_QUERY 0x01
#define DRM_CASTKMS_MONITOR_ATTACH 0x02
#define DRM_CASTKMS_MONITOR_DETACH 0x03
#define DRM_CASTKMS_RENDERER_QUERY 0x04

/* This is an enum so that Rust bindgen resolves the ioctl values. */
enum {
	DRM_IOCTL_CASTKMS_CREATE_MONITOR_CONTROL =
		DRM_IOWR(DRM_COMMAND_BASE + DRM_CASTKMS_CREATE_MONITOR_CONTROL,
			 struct drm_castkms_create_monitor_control),
	DRM_IOCTL_CASTKMS_CREATE_RENDERER_CONTROL =
		DRM_IOW(DRM_COMMAND_BASE + DRM_CASTKMS_CREATE_RENDERER_CONTROL,
			 struct drm_castkms_create_renderer_control),
	DRM_IOCTL_CASTKMS_MONITOR_QUERY =
		DRM_IOR(DRM_COMMAND_BASE + DRM_CASTKMS_MONITOR_QUERY,
			struct drm_castkms_monitor_query),
	DRM_IOCTL_CASTKMS_MONITOR_ATTACH =
		DRM_IOW(DRM_COMMAND_BASE + DRM_CASTKMS_MONITOR_ATTACH,
			struct drm_castkms_monitor_attach),
	DRM_IOCTL_CASTKMS_MONITOR_DETACH =
		DRM_IOW(DRM_COMMAND_BASE + DRM_CASTKMS_MONITOR_DETACH,
			struct drm_castkms_monitor_detach),
	DRM_IOCTL_CASTKMS_RENDERER_QUERY =
		DRM_IOR(DRM_COMMAND_BASE + DRM_CASTKMS_RENDERER_QUERY,
			struct drm_castkms_renderer_query),
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
