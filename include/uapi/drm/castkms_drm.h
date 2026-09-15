/* SPDX-License-Identifier: MIT */
#ifndef _UAPI_CASTKMS_DRM_H_
#define _UAPI_CASTKMS_DRM_H_

#include "drm.h"

#include <linux/types.h>

#define DRM_CASTKMS_MONITOR_CONTROL_VERSION 1
#define DRM_CASTKMS_MONITOR_MAX_EDID_SIZE (256U * 128U)
#define DRM_CASTKMS_RENDERER_VERSION 7

/*
 * Request-only unsigned CRTC property. Zero means an ordinary update. A nonzero
 * value identifies a pending transition on that output; atomic checking and
 * final installation validate both contracts. TEST_ONLY installs no gate.
 * Successful installation binds the gate to the accepted configuration.
 * Readback is always zero and state duplication never carries the tag forward.
 * A token is metadata, not permission to render or access source buffers.
 */
#define DRM_CASTKMS_TRANSITION_PROPERTY "CASTKMS_TRANSITION"

#define DRM_CASTKMS_CAPABILITY_VERSION 1
#define DRM_CASTKMS_CAPABILITY_HOST 1
#define DRM_CASTKMS_CAPABILITY_RENDERER 2
#define DRM_CASTKMS_CAPABILITY_MAX_FORMATS 256
#define DRM_CASTKMS_CAPABILITY_MAX_BYTES (128U + 32U * 256U)
#define DRM_CASTKMS_CAPABILITY_QUERY_MAX_BYTES \
	(72U + 2U * DRM_CASTKMS_CAPABILITY_MAX_BYTES)

#define DRM_CASTKMS_CAPABILITY_CROP (1U << 0)
#define DRM_CASTKMS_CAPABILITY_FRACTIONAL (1U << 1)
#define DRM_CASTKMS_CAPABILITY_POSITION (1U << 2)
#define DRM_CASTKMS_CAPABILITY_SCALE (1U << 3)
#define DRM_CASTKMS_CAPABILITY_SRGB (1U << 4)
#define DRM_CASTKMS_CAPABILITY_PLANE_MATRIX (1U << 5)
#define DRM_CASTKMS_CAPABILITY_OUTPUT_MATRIX (1U << 6)

#define DRM_CASTKMS_CAPABILITY_NATIVE (1U << 0)
#define DRM_CASTKMS_CAPABILITY_IMPORTED (1U << 1)
#define DRM_CASTKMS_CAPABILITY_EXPLICIT_MODIFIER (1U << 2)

/*
 * Native-endian immutable whole-scene contract. Exactly format_count records
 * follow this 128-byte header. Unknown flags and reserved fields must be zero.
 * HOST selects the fixed HOST-v1 policy: all fields after kind must be zero.
 * RENDERER limits apply to every role; advertise the intersection if roles have
 * different restrictions. Dimensions and scale limits are positive; scales are
 * inclusive unsigned 16.16 source/destination ratios. Roles are primary,
 * overlay and cursor. YUV masks use bit positions from the scene encoding.
 * Sampling is nearest-neighbor, blending is premultiplied source-over and
 * stacking follows the scene description. A profile grants no buffer access.
 */
struct drm_castkms_capability_profile {
	__u32 version;
	__u32 kind;
	__u32 flags;
	__u32 format_count;
	__u32 max_output[2];
	__u32 max_source[2];
	__u32 min_scale;
	__u32 max_scale;
	__u32 max_layers;
	__u32 max_roles[3];
	__u32 max_color_operations;
	__u32 max_lut_entries;
	__u32 yuv_encodings;
	__u32 yuv_ranges;
	__u32 reserved[14];
};

/*
 * Exact fourcc/modifier/plane-count tuple. Without EXPLICIT_MODIFIER, modifier
 * must be zero and denotes implicit layout, distinct from explicit LINEAR.
 * At least one provenance flag is required. Alignments are positive powers of
 * two and apply to each memory plane. Duplicate tuples are rejected.
 */
struct drm_castkms_capability_format {
	__u32 fourcc;
	__u32 plane_count;
	__u64 modifier;
	__u32 flags;
	__u32 pitch_alignment;
	__u32 offset_alignment;
	__u32 max_pitch;
};

/*
 * Register one immutable target for an existing BEGIN_TAKEOVER candidate.
 * profile points to exactly profile_size bytes of capability encoding.
 * result points to drm_castkms_renderer_profile_result; flags must be zero.
 * HOST requires no userspace probe; a RENDERER target requires normal probe
 * completion. Registration does not change KMS acceptance. Include transition
 * in CASTKMS_TRANSITION on an ordinary compatible atomic update, then invoke
 * COMMIT_TAKEOVER. ABORT_TAKEOVER cancels the pending profile and gate.
 * A reply-copy fault may leave registration committed: query to reconcile it.
 * Repeating registration while a proposal exists returns EBUSY, not a new token.
 */
struct drm_castkms_renderer_register_profile {
	__u64 candidate_id;
	__u64 profile;
	__u64 result;
	__u32 profile_size;
	__u32 flags;
};

struct drm_castkms_renderer_profile_result {
	__u64 transition;
	__u64 capability_generation;
	__u64 execution_generation;
	__u64 reserved;
};

#define DRM_CASTKMS_CAPABILITY_PENDING (1U << 0)
#define DRM_CASTKMS_CAPABILITY_GATED (1U << 1)

/*
 * A coherent native-endian snapshot followed by active and optional pending
 * capability encodings. Offsets are relative to this 72-byte header. Without a
 * live pending transition, pending fields, transition and pending flags are zero.
 * Capability generation identifies an immutable contract; validation_epoch also
 * changes when a gate is installed or lifted. Neither identifies source content.
 * Size includes the entire snapshot. Output fields grant no continuing authority.
 */
struct drm_castkms_renderer_capabilities {
	__u32 version;
	__u32 size;
	__u32 execution_profile;
	__u32 flags;
	__u64 execution_generation;
	__u64 active_generation;
	__u64 pending_generation;
	__u64 transition;
	__u64 validation_epoch;
	__u32 active_offset;
	__u32 active_size;
	__u32 pending_offset;
	__u32 pending_size;
};

/*
 * result points to capacity writable bytes. Capacity must be at least 72.
 * On ENOSPC only the header is written, including the required total size;
 * retry for a fresh coherent snapshot. Copy faults may partially write output.
 * flags and reserved must be zero. Queries work with disabled video, but still
 * require live output authority. They never reserve a transition or source read.
 */
struct drm_castkms_renderer_query_capabilities {
	__u64 result;
	__u32 capacity;
	__u32 flags;
	__u64 reserved;
};

#define DRM_CASTKMS_RENDERER_PROBE_PRIVATE 1
#define DRM_CASTKMS_RENDERER_PROBE_STARTUP_IMAGE 2

#define DRM_CASTKMS_RENDERER_RELEASE_NO_ACCESS 1
#define DRM_CASTKMS_RENDERER_RELEASE_CPU_DONE 2
#define DRM_CASTKMS_RENDERER_RELEASE_SUBMITTED 3
#define DRM_CASTKMS_RENDERER_MAX_PLANES 4

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

/**
 * struct drm_castkms_renderer_source_plane - one source memory plane
 * @dma_buf_fd: returned close-on-exec DMA-BUF descriptor
 * @pitch: byte stride for this plane
 * @offset: byte offset to this plane in the DMA-BUF
 * @reserved: returned as zero
 */
struct drm_castkms_renderer_source_plane {
	__s32 dma_buf_fd;
	__u32 pitch;
	__u32 offset;
	__u32 reserved;
};

/**
 * struct drm_castkms_renderer_source - one claimed scene source
 * @job_id: nonzero name for RENDERER_RELEASE_SOURCE
 * @content_serial: nonzero content identity within this output
 * @modifier: framebuffer DRM_FORMAT_MOD_* value, or DRM_FORMAT_MOD_INVALID
 * @format: framebuffer DRM_FORMAT_* value
 * @width: framebuffer width in pixels
 * @height: framebuffer height in pixels
 * @plane_count: number of initialized entries in @planes
 * @producer_fd: close-on-exec sync_file for captured producer dependencies,
 * or -1 when no native wait is needed
 * @reserved: returned as zero
 * @source: source rectangle in unsigned 16.16 coordinates
 * @destination: destination dimensions in output pixels
 * @output: complete output dimensions in pixels
 * @planes: source planes; unused entries contain fd -1 and zero metadata
 *
 * Each descriptor names ordinary, non-revocable DMA-BUF storage. The job's
 * source-read claim separately governs access until RELEASE_SOURCE. Retaining
 * a descriptor after release does not authorize another source read. The
 * producer sync_file covers dependencies captured when KMS accepted the scene;
 * the renderer must wait for it before reading any source plane.
 */
struct drm_castkms_renderer_source {
	__u64 job_id;
	__u64 content_serial;
	__u64 modifier;
	__u32 format;
	__u32 width;
	__u32 height;
	__u32 plane_count;
	__s32 producer_fd;
	__u32 reserved;
	__u32 source[4];
	__u32 destination[2];
	__u32 output[2];
	struct drm_castkms_renderer_source_plane
		planes[DRM_CASTKMS_RENDERER_MAX_PLANES];
};

/**
 * struct drm_castkms_renderer_dequeue_source - publish the next changed scene
 * @result: pointer to writable struct drm_castkms_renderer_source storage
 * @flags: must be zero
 * @reserved: must be zero
 *
 * Only an activated renderer may dequeue. At most one source job is published
 * at a time. EBUSY means that job still needs release; ENODATA means the
 * current content was already published or the output is blank. Success copies
 * all metadata before installing plane descriptors. Failure installs no
 * descriptor; output memory may have been partially written and must not be
 * used.
 */
struct drm_castkms_renderer_dequeue_source {
	__u64 result;
	__u32 flags;
	__u32 reserved[3];
};

/**
 * struct drm_castkms_renderer_release_source - resolve one source read
 * @job_id: job returned by RENDERER_DEQUEUE_SOURCE
 * @completion_fd: sync_file descriptor for SUBMITTED, otherwise -1
 * @kind: one DRM_CASTKMS_RENDERER_RELEASE_* value
 * @flags: must be zero
 * @reserved: must be zero
 *
 * NO_ACCESS promises no source access occurred. CPU_DONE promises all CPU
 * access and coherency operations ended. SUBMITTED transfers a native fence
 * covering every submitted source access and promises no later submission
 * under this job. Repeating the accepted release for the latest job succeeds.
 */
struct drm_castkms_renderer_release_source {
	__u64 job_id;
	__s32 completion_fd;
	__u32 kind;
	__u32 flags;
	__u32 reserved[3];
};

/* Complete-scene stream, native byte order. All records are eight-byte aligned.
 * DEQUEUE_SCENE shares the source queue and RELEASE_SOURCE lifetime contract.
 * The result consists of a scene header, layer records with their color records,
 * then output color records. Layer order is back-to-front, with zpos ties in
 * KMS plane creation order. Source rectangles use unsigned 16.16 pixels;
 * signed destination positions permit clipping. Sampling is nearest-neighbor.
 * Layers use premultiplied pixel alpha (opaque for formats without alpha), source
 * over an opaque black background. Plane color precedes blending; output color
 * follows blending. All unused memory-plane records contain fd -1 and zeros.
 * The producer fd covers all layers and must complete successfully before any
 * source read; -1 denotes no outstanding producer fence. Retaining ordinary
 * DMA-BUF fds does not authorize reads after RELEASE_SOURCE.
 * No descriptor is installed on failure, even after a partial metadata copy.
 * Allocate SCENE_MAX_BYTES for the result; a smaller capacity may return
 * ENOSPC without consuming the scene. Flags and reserved fields must be zero.
 * Empty/unchanged scenes return ENODATA, and an outstanding job returns EBUSY.
 */
#define DRM_CASTKMS_RENDERER_SCENE_VERSION 1
#define DRM_CASTKMS_RENDERER_SCENE_MAX_BYTES 65536
#define DRM_CASTKMS_RENDERER_SCENE_MAX_LAYERS 24
#define DRM_CASTKMS_RENDERER_SCENE_MAX_COLOR_OPS 16
#define DRM_CASTKMS_RENDERER_LAYER_PRIMARY 0
#define DRM_CASTKMS_RENDERER_LAYER_OVERLAY 1
#define DRM_CASTKMS_RENDERER_LAYER_CURSOR 2
#define DRM_CASTKMS_RENDERER_COLOR_BYPASS 0
#define DRM_CASTKMS_RENDERER_COLOR_SRGB_EOTF 1
#define DRM_CASTKMS_RENDERER_COLOR_SRGB_INVERSE_EOTF 2
#define DRM_CASTKMS_RENDERER_COLOR_MATRIX 3
#define DRM_CASTKMS_RENDERER_COLOR_LUT 4

struct drm_castkms_renderer_dequeue_scene {
	__u64 result;
	__u32 capacity;
	__u32 flags;
	__u64 reserved;
};

struct drm_castkms_renderer_scene {
	__u32 version;
	__u32 bytes;
	__u64 job_id;
	__u64 content_serial;
	__u32 width;
	__u32 height;
	__u32 layer_count;
	__s32 producer_fd;
	__u32 output_color_count;
	__u32 reserved;
};

struct drm_castkms_renderer_layer {
	__u32 bytes; /* Includes following color records. */
	__u32 kind;
	__u32 zpos;
	__u32 format;
	__u64 modifier;
	__u32 width;
	__u32 height;
	__u32 source[4];
	__s32 position[2];
	__u32 destination[2];
	__u32 color_encoding; /* 0 BT.601, 1 BT.709, 2 BT.2020 nonconstant. */
	__u32 color_range; /* 0 limited, 1 full. */
	__u32 plane_count;
	__u32 color_count;
	struct drm_castkms_renderer_source_plane planes[4];
};

/* Each color record starts with kind and payload_bytes. Curves and bypass
 * have no payload. MATRIX has twelve u64 S31.32 sign-magnitude coefficients
 * (three rows of four, including offsets); LUT has 1..256 entries containing
 * u16 red, green, blue, zero. LUT input/output range is 0..65535 with linear
 * interpolation. Plane matrices retain signed extended range between steps;
 * curves and the pipeline output clamp to 0..65535. Matrix offsets use the
 * same channel units, not normalized 0..1 units. Output operations are applied
 * in degamma-LUT, matrix, gamma-LUT order, omitting absent operations.
 */
struct drm_castkms_renderer_color {
	__u32 kind;
	__u32 payload_bytes;
};

/**
 * struct drm_castkms_audio_files - independently owned audio endpoints
 * @audio_fd: Read-only interleaved PCM stream; no ALSA capture device is created.
 * @revoke_fd: Closing its last reference revokes the stream.
 *
 * Both files are close-on-exec. Closing the issuing DRM file also revokes audio.
 * Neither endpoint grants image capture, monitor control or renderer access.
 */
struct drm_castkms_audio_files {
	__s32 audio_fd;
	__s32 revoke_fd;
};

/**
 * struct drm_castkms_create_audio_capture - authorize one attachment's audio
 * @crtc_id: CRTC controlled by the current top-level DRM master.
 * @connector_id: Connector paired with that CRTC, with an attached audio sink.
 * @files: Userspace address of struct drm_castkms_audio_files, output only.
 * @flags: Zero, or DRM_CASTKMS_AUDIO_NONBLOCK for nonblocking reads.
 * @reserved: Must be zero.
 *
 * At most one live stream may capture an attachment. Master loss, creator close,
 * revocation, detach or device removal terminates the stream and discards queued
 * samples. An old file never follows a replacement attachment. Capture operates
 * independently of the selected video renderer. Disabling the CRTC suspends
 * delivery without revoking the capability. Files are installed only after
 * the complete result has been copied successfully.
 */
struct drm_castkms_create_audio_capture {
	__u32 crtc_id;
	__u32 connector_id;
	__u64 files;
	__u32 flags;
	__u32 reserved[3];
};

#define DRM_CASTKMS_AUDIO_VERSION 1
#define DRM_CASTKMS_AUDIO_NONBLOCK (1U << 0)
#define DRM_CASTKMS_AUDIO_S16_LE 1

/**
 * struct drm_castkms_audio_query - fixed PCM stream description and loss counter
 * @version: DRM_CASTKMS_AUDIO_VERSION.
 * @format: DRM_CASTKMS_AUDIO_S16_LE, stereo interleaved signed 16-bit little endian.
 * @rate: Sample rate in frames per second (48000).
 * @channels: Channels per frame (2).
 * @frame_bytes: Bytes per interleaved frame (4).
 * @reserved: Zero.
 * @buffer_frames: Maximum queued frames (65536).
 * @dropped_frames: Saturating count of frames discarded due to scheduling or overflow.
 *
 * read() returns whole frames, including silence during idle playback while
 * the CRTC is active. An inactive CRTC supplies no frames and queued samples
 * are discarded on activity transitions. ALSA playback interrupted by a
 * modeset must be prepared again. Nonzero reads smaller than one frame fail
 * with EINVAL; an empty nonblocking stream returns
 * EAGAIN. poll() reports readable samples or terminal POLLHUP|POLLERR.
 */
struct drm_castkms_audio_query {
	__u32 version;
	__u32 format;
	__u32 rate;
	__u32 channels;
	__u32 frame_bytes;
	__u32 reserved;
	__u64 buffer_frames;
	__u64 dropped_frames;
};

#define DRM_CASTKMS_CREATE_AUDIO_CAPTURE 0x02
#define DRM_CASTKMS_AUDIO_QUERY 0x01
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
#define DRM_CASTKMS_RENDERER_DEQUEUE_SOURCE 0x0a
#define DRM_CASTKMS_RENDERER_RELEASE_SOURCE 0x0b
#define DRM_CASTKMS_RENDERER_DEQUEUE_SCENE 0x0c
#define DRM_CASTKMS_RENDERER_REGISTER_PROFILE 0x0d
#define DRM_CASTKMS_RENDERER_QUERY_CAPABILITIES 0x0e

/* This is an enum so that Rust bindgen resolves the ioctl values. */
enum {
	DRM_IOCTL_CASTKMS_RENDERER_REGISTER_PROFILE =
		DRM_IOW(DRM_COMMAND_BASE + DRM_CASTKMS_RENDERER_REGISTER_PROFILE,
			struct drm_castkms_renderer_register_profile),
	DRM_IOCTL_CASTKMS_RENDERER_QUERY_CAPABILITIES =
		DRM_IOW(DRM_COMMAND_BASE + DRM_CASTKMS_RENDERER_QUERY_CAPABILITIES,
			struct drm_castkms_renderer_query_capabilities),
	DRM_IOCTL_CASTKMS_RENDERER_DEQUEUE_SCENE =
		DRM_IOW(DRM_COMMAND_BASE + DRM_CASTKMS_RENDERER_DEQUEUE_SCENE,
			struct drm_castkms_renderer_dequeue_scene),
	DRM_IOCTL_CASTKMS_CREATE_AUDIO_CAPTURE =
		DRM_IOW(DRM_COMMAND_BASE + DRM_CASTKMS_CREATE_AUDIO_CAPTURE,
			struct drm_castkms_create_audio_capture),
	DRM_IOCTL_CASTKMS_AUDIO_QUERY =
		DRM_IOR(DRM_COMMAND_BASE + DRM_CASTKMS_AUDIO_QUERY,
			struct drm_castkms_audio_query),
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
	DRM_IOCTL_CASTKMS_RENDERER_DEQUEUE_SOURCE =
		DRM_IOW(DRM_COMMAND_BASE + DRM_CASTKMS_RENDERER_DEQUEUE_SOURCE,
			 struct drm_castkms_renderer_dequeue_source),
	DRM_IOCTL_CASTKMS_RENDERER_RELEASE_SOURCE =
		DRM_IOW(DRM_COMMAND_BASE + DRM_CASTKMS_RENDERER_RELEASE_SOURCE,
			 struct drm_castkms_renderer_release_source),
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
