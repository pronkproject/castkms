/* SPDX-License-Identifier: MIT */
#ifndef _UAPI_CASTKMS_DRM_H_
#define _UAPI_CASTKMS_DRM_H_

#include "drm.h"

#include <linux/types.h>

#define DRM_CASTKMS_MONITOR_CONTROL_VERSION 1
#define DRM_CASTKMS_MONITOR_MAX_EDID_SIZE (256U * 128U)
#define DRM_CASTKMS_RENDERER_VERSION 4

#define DRM_CASTKMS_RENDERER_PROBE_PRIVATE 1
#define DRM_CASTKMS_RENDERER_PROBE_STARTUP_IMAGE 2

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

/**
 * struct drm_castkms_renderer_takeover - published candidate description
 * @candidate_id: nonzero name for later operations on this candidate
 * @execution_generation: execution generation observed while reserving
 * @profile: current DRM_CASTKMS_EXECUTION_* profile
 * @width: current output width in pixels
 * @height: current output height in pixels
 * @refresh_millihz: current display refresh in millihertz
 * @mode_flags: current DRM_MODE_FLAG_* values
 * @reserved: returned as zero
 *
 * This is configuration metadata, not permission to access source pixels.
 */
struct drm_castkms_renderer_takeover {
	__u64 candidate_id;
	__u64 execution_generation;
	__u32 profile;
	__u32 width;
	__u32 height;
	__u32 refresh_millihz;
	__u32 mode_flags;
	__u32 reserved;
};

/**
 * struct drm_castkms_renderer_begin_takeover - reserve candidate startup
 * @expected_generation: current HOST execution generation from query
 * @result: pointer to writable struct drm_castkms_renderer_takeover storage
 * @flags: must be zero
 * @reserved: must be zero
 *
 * Only one candidate may be reserved for an output. HOST execution and capture
 * remain active. Success returns a candidate description after all fallible
 * user-memory access. Failure publishes no candidate; output memory may have
 * been partially written and must not be used.
 */
struct drm_castkms_renderer_begin_takeover {
	__u64 expected_generation;
	__u64 result;
	__u32 flags;
	__u32 reserved[3];
};

/**
 * struct drm_castkms_renderer_abort_takeover - release one candidate
 * @candidate_id: candidate returned by BEGIN_TAKEOVER
 * @flags: must be zero
 * @reserved: must be zero
 *
 * Aborting never changes the active execution profile.
 */
struct drm_castkms_renderer_abort_takeover {
	__u64 candidate_id;
	__u32 flags;
	__u32 reserved;
};

/**
 * struct drm_castkms_renderer_snapshot - independent HOST startup image
 * @dma_buf_fd: returned close-on-exec, read-only DMA-BUF descriptor
 * @format: returned DRM_FORMAT_* value
 * @modifier: returned DRM_FORMAT_MOD_* value
 * @width: returned width in pixels
 * @height: returned height in pixels
 * @pitch: returned byte stride
 * @offset: returned first-pixel byte offset; currently zero
 * @content_serial: historical nonzero content identity, or zero for a blank image
 * @flags: returned as zero
 * @reserved: returned as zero
 *
 * The backing allocation is a fresh immutable copy. It is never a compositor
 * source or reusable HOST image, and retaining it cannot delay source release.
 */
struct drm_castkms_renderer_snapshot {
	__s32 dma_buf_fd;
	__u32 format;
	__u64 modifier;
	__u32 width;
	__u32 height;
	__u32 pitch;
	__u32 offset;
	__u64 content_serial;
	__u32 flags;
	__u32 reserved;
};

/**
 * struct drm_castkms_renderer_get_snapshot - copy an optional HOST startup image
 * @candidate_id: active candidate returned by BEGIN_TAKEOVER
 * @result: pointer to writable struct drm_castkms_renderer_snapshot storage
 * @flags: must be zero
 * @reserved: must be zero
 *
 * The operation copies only the newest retained HOST result that still belongs
 * to the candidate's display and authority interval. It returns ENODATA when no
 * eligible result exists. Success copies the result before installing its
 * descriptor. Failure installs no descriptor; output memory may have been
 * partially written and must not be used.
 */
struct drm_castkms_renderer_get_snapshot {
	__u64 candidate_id;
	__u64 result;
	__u32 flags;
	__u32 reserved[3];
};

/**
 * struct drm_castkms_renderer_submit_probe - publish candidate test work
 * @candidate_id: active candidate returned by BEGIN_TAKEOVER
 * @completion_fd: sync_file for submitted native work, or -1 when already done
 * @source: one DRM_CASTKMS_RENDERER_PROBE_* value
 * @flags: must be zero
 * @reserved: must be zero
 *
 * A private probe uses only renderer-owned storage and carries no display
 * content identity. A startup-image probe additionally requires one successful
 * GET_SNAPSHOT on the same candidate; the kernel retains the identity that it
 * delivered rather than accepting content metadata from userspace.
 *
 * The completion fence must cover every native access made by the probe. A
 * value of -1 declares that all access completed before this ioctl. Success
 * records exactly one submission without activating delegated execution or
 * granting access to live compositor sources. Fence failure later makes the
 * probe unsuccessful.
 */
struct drm_castkms_renderer_submit_probe {
	__u64 candidate_id;
	__s32 completion_fd;
	__u32 source;
	__u32 flags;
	__u32 reserved[3];
};

/**
 * struct drm_castkms_renderer_commit_takeover - activate delegated execution
 * @candidate_id: candidate whose submitted probe completed successfully
 * @flags: must be zero
 * @reserved: must be zero
 *
 * Success atomically transfers the candidate into active-renderer ownership,
 * publishes a new GPU execution generation, and closes new HOST source-read
 * admission. Work admitted before the transition retires normally.
 *
 * Repeating the operation for the same active candidate succeeds so a caller
 * can reconcile a lost reply. A pending probe returns EAGAIN; a failed probe
 * returns its exact completion error. Other stale candidates are rejected.
 */
struct drm_castkms_renderer_commit_takeover {
	__u64 candidate_id;
	__u32 flags;
	__u32 reserved;
};

#define DRM_CASTKMS_CREATE_MONITOR_CONTROL 0x00
#define DRM_CASTKMS_CREATE_RENDERER_CONTROL 0x01
#define DRM_CASTKMS_MONITOR_QUERY 0x01
#define DRM_CASTKMS_MONITOR_ATTACH 0x02
#define DRM_CASTKMS_MONITOR_DETACH 0x03
#define DRM_CASTKMS_RENDERER_QUERY 0x04
#define DRM_CASTKMS_RENDERER_BEGIN_TAKEOVER 0x05
#define DRM_CASTKMS_RENDERER_ABORT_TAKEOVER 0x06
#define DRM_CASTKMS_RENDERER_GET_SNAPSHOT 0x07
#define DRM_CASTKMS_RENDERER_SUBMIT_PROBE 0x08
#define DRM_CASTKMS_RENDERER_COMMIT_TAKEOVER 0x09

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
	DRM_IOCTL_CASTKMS_RENDERER_BEGIN_TAKEOVER =
		DRM_IOW(DRM_COMMAND_BASE + DRM_CASTKMS_RENDERER_BEGIN_TAKEOVER,
			struct drm_castkms_renderer_begin_takeover),
	DRM_IOCTL_CASTKMS_RENDERER_ABORT_TAKEOVER =
		DRM_IOW(DRM_COMMAND_BASE + DRM_CASTKMS_RENDERER_ABORT_TAKEOVER,
			struct drm_castkms_renderer_abort_takeover),
	DRM_IOCTL_CASTKMS_RENDERER_GET_SNAPSHOT =
		DRM_IOW(DRM_COMMAND_BASE + DRM_CASTKMS_RENDERER_GET_SNAPSHOT,
			struct drm_castkms_renderer_get_snapshot),
	DRM_IOCTL_CASTKMS_RENDERER_SUBMIT_PROBE =
		DRM_IOW(DRM_COMMAND_BASE + DRM_CASTKMS_RENDERER_SUBMIT_PROBE,
			 struct drm_castkms_renderer_submit_probe),
	DRM_IOCTL_CASTKMS_RENDERER_COMMIT_TAKEOVER =
		DRM_IOW(DRM_COMMAND_BASE + DRM_CASTKMS_RENDERER_COMMIT_TAKEOVER,
			 struct drm_castkms_renderer_commit_takeover),
};

#define DRM_CASTKMS_EXECUTION_VERSION 1
#define DRM_CASTKMS_EXECUTION_HOST_V1 1
#define DRM_CASTKMS_EXECUTION_GPU_V1 2

/**
 * struct drm_castkms_execution - CASTKMS_EXECUTION connector blob
 * @version: Description layout version, DRM_CASTKMS_EXECUTION_VERSION.
 * @profile: Active DRM_CASTKMS_EXECUTION_* profile.
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
 *
 * GPU_V1 identifies one activated userspace renderer. New HOST source reads are
 * rejected while that renderer owns execution; work admitted before activation
 * retires normally. Renderer operations define the accepted delegated work and
 * completion contract independently of final-image capture permission.
 */
struct drm_castkms_execution {
	__u32 version;
	__u32 profile;
	__u64 generation;
};

#endif
