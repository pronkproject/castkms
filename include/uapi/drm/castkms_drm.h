/* SPDX-License-Identifier: MIT */
#ifndef _UAPI_CASTKMS_DRM_H_
#define _UAPI_CASTKMS_DRM_H_

#include "drm.h"

#include <linux/types.h>

#define DRM_CASTKMS_MONITOR_CONTROL_VERSION 1
#define DRM_CASTKMS_MONITOR_MAX_EDID_SIZE (256U * 128U)
#define DRM_CASTKMS_RENDERER_VERSION 1

#define DRM_CASTKMS_RENDERER_CONSTRAINTS_VERSION 1
#define DRM_CASTKMS_RENDERER_CONSTRAINTS_KIND 1
#define DRM_CASTKMS_RENDERER_CONSTRAINTS_MAX_FORMATS 256
#define DRM_CASTKMS_RENDERER_CONSTRAINTS_HEADER_BYTES 128U
#define DRM_CASTKMS_RENDERER_CONSTRAINTS_FORMAT_BYTES 56U
#define DRM_CASTKMS_RENDERER_CONSTRAINTS_MAX_BYTES \
	(DRM_CASTKMS_RENDERER_CONSTRAINTS_HEADER_BYTES + \
	 DRM_CASTKMS_RENDERER_CONSTRAINTS_FORMAT_BYTES * \
	 DRM_CASTKMS_RENDERER_CONSTRAINTS_MAX_FORMATS)

#define DRM_CASTKMS_RENDERER_CONSTRAINTS_CROP (1U << 0)
#define DRM_CASTKMS_RENDERER_CONSTRAINTS_FRACTIONAL (1U << 1)
#define DRM_CASTKMS_RENDERER_CONSTRAINTS_POSITION (1U << 2)
#define DRM_CASTKMS_RENDERER_CONSTRAINTS_SCALE (1U << 3)
#define DRM_CASTKMS_RENDERER_CONSTRAINTS_SRGB (1U << 4)
#define DRM_CASTKMS_RENDERER_CONSTRAINTS_PLANE_MATRIX (1U << 5)
#define DRM_CASTKMS_RENDERER_CONSTRAINTS_OUTPUT_MATRIX (1U << 6)

#define DRM_CASTKMS_RENDERER_CONSTRAINTS_FORMAT_NATIVE (1U << 0)
#define DRM_CASTKMS_RENDERER_CONSTRAINTS_FORMAT_IMPORTED (1U << 1)
#define DRM_CASTKMS_RENDERER_CONSTRAINTS_FORMAT_EXPLICIT_MODIFIER (1U << 2)

#define DRM_CASTKMS_RENDERER_CONSTRAINTS_ROLE_PRIMARY (1U << 0)
#define DRM_CASTKMS_RENDERER_CONSTRAINTS_ROLE_OVERLAY (1U << 1)
#define DRM_CASTKMS_RENDERER_CONSTRAINTS_ROLE_CURSOR (1U << 2)

/* Job-plane values and the corresponding capability-mask bits share one namespace. */
#define DRM_CASTKMS_YUV_ENCODING_BT601 0
#define DRM_CASTKMS_YUV_ENCODING_BT709 1
#define DRM_CASTKMS_YUV_ENCODING_BT2020 2 /* Nonconstant luminance. */
#define DRM_CASTKMS_YUV_RANGE_LIMITED 0
#define DRM_CASTKMS_YUV_RANGE_FULL 1
#define DRM_CASTKMS_RENDERER_CONSTRAINTS_YUV_ENCODING_BT601 (1U << DRM_CASTKMS_YUV_ENCODING_BT601)
#define DRM_CASTKMS_RENDERER_CONSTRAINTS_YUV_ENCODING_BT709 (1U << DRM_CASTKMS_YUV_ENCODING_BT709)
#define DRM_CASTKMS_RENDERER_CONSTRAINTS_YUV_ENCODING_BT2020 (1U << DRM_CASTKMS_YUV_ENCODING_BT2020)
#define DRM_CASTKMS_RENDERER_CONSTRAINTS_YUV_RANGE_LIMITED (1U << DRM_CASTKMS_YUV_RANGE_LIMITED)
#define DRM_CASTKMS_RENDERER_CONSTRAINTS_YUV_RANGE_FULL (1U << DRM_CASTKMS_YUV_RANGE_FULL)

#define DRM_CASTKMS_RENDERER_CREATE_ADMIN (1U << 0)

/*
 * Native-endian immutable whole-state execution contract. Exactly format_count records
 * follow this 128-byte header. Unknown flags and reserved fields must be zero.
 * kind must be DRM_CASTKMS_RENDERER_CONSTRAINTS_KIND. Fixed default constraints
 * are discovered through generic KMS listing, not supplied by the worker.
 * Limits other than format choices apply to every role; advertise their
 * intersection if roles have different restrictions. Dimensions and scale limits are
 * inclusive unsigned 16.16 source/destination ratios. Roles are primary,
 * overlay and cursor. Without the SCALE flag, both scale limits must equal
 * 1.0 (1 << 16). YUV masks use bit positions from the job encoding.
 * Sampling is nearest-neighbor, blending is premultiplied source-over and
 * stacking follows the job description. Constraints grant no buffer access.
 * min_output/min_source and max_output/max_source bound width and height
 * inclusively.
 * RENDERER bounds must be positive with min <= max on each axis. Equal bounds
 * express exact geometry, e.g. min_output == max_output for a fixed-size pool.
 * Source bounds describe full framebuffers, not cropped extents; output bounds
 * describe the composed image, not an individual plane's CRTC rectangle.
 * max_color_operations applies independently to each plane color pipeline and
 * to the output color pipeline.
 * Header and 56-byte format-record layouts are fixed within a version;
 * new record fields require a new version.
 */
struct drm_castkms_renderer_constraints {
	__u32 version;
	__u32 kind;
	__u32 flags;
	__u32 format_count;
	__u32 max_output[2];
	__u32 max_source[2];
	__u32 min_scale;
	__u32 max_scale;
	__u32 max_planes;
	__u32 max_roles[3];
	__u32 max_color_operations;
	__u32 max_lut_entries;
	__u32 yuv_encodings;
	__u32 yuv_ranges;
	__u32 min_output[2];
	__u32 min_source[2];
	__u32 reserved[10];
};

/*
 * Exact fourcc/modifier/memory-plane-count tuple. Without EXPLICIT_MODIFIER, modifier
 * must be zero and denotes implicit layout, distinct from explicit LINEAR.
 * At least one provenance flag is required. width_alignment and
 * height_alignment are positive powers of two in pixels. The byte alignments
 * are positive powers of two and apply to each memory plane. roles is a
 * nonempty mask of DRM_CASTKMS_RENDERER_CONSTRAINTS_ROLE_* values. Each tuple
 * applies only to those plane roles; max_roles can independently prohibit a
 * role. min_pitch and
 * max_pitch are inclusive and contain at least one aligned value. Only tuples
 * with at least one aligned size inside the declared source bounds, and whose
 * pitch interval admits the generic DRM minimum at the smallest such width,
 * can be published. Modifier- and driver-specific rules may be stricter.
 * Duplicate tuples are rejected. reserved must be zero.
 */
struct drm_castkms_renderer_constraints_format {
	__u32 fourcc;
	__u32 memory_plane_count;
	__u64 modifier;
	__u32 flags;
	__u32 roles;
	__u32 width_alignment;
	__u32 height_alignment;
	__u32 pitch_alignment;
	__u32 offset_alignment;
	__u32 min_pitch;
	__u32 max_pitch;
	__u32 reserved[2];
};

#define DRM_CASTKMS_RENDERER_RELEASE_NO_ACCESS 1
#define DRM_CASTKMS_RENDERER_RELEASE_CPU_DONE 2
#define DRM_CASTKMS_RENDERER_RELEASE_SUBMITTED 3
#define DRM_CASTKMS_RENDERER_MAX_MEMORY_PLANES 4

/**
 * struct drm_castkms_create_monitor_control - create virtual monitor control
 * @connector_id: DRM object ID of the virtual connector
 * @flags: must be zero
 * @files: pointer to writable struct drm_castkms_monitor_files storage
 * @reserved: must be zero
 *
 * The calling DRM file must be the current master and hold the connector.
 * Only one monitor-control file may exist for a connector. Connectors remain
 * disconnected until explicitly attached. Final close of the control file,
 * or close of the revocation file, disconnects the monitor. The control
 * capability remains valid across DRM master changes and may be transferred
 * like any other file descriptor. The issuer
 * retains the revocation file.
 * All request fields are input. Failure installs no descriptors and does not
 * change monitor state; partially copied output must be ignored.
 */
struct drm_castkms_create_monitor_control {
	__u32 connector_id;
	__u32 flags;
	__u64 files;
	__u64 reserved[2];
};

/* Returned close-on-exec descriptors; monitor control and revocation. */
struct drm_castkms_monitor_files {
	__s32 control_fd;
	__s32 revoke_fd;
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
 * @edid_size: complete EDID size, or zero for fallback modes with 1080p preferred
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
 * Final revoker close stops new admission through every duplicate renderer
 * descriptor. RELEASE_JOB and UNREGISTER_IMAGE remain available for cleanup
 * until final renderer-file close, which ends the reporting channel. Closing
 * one duplicated renderer descriptor releases only that file reference.
 */
struct drm_castkms_renderer_files {
	__s32 renderer_fd;
	__s32 revoke_fd;
};

/**
 * struct drm_castkms_create_renderer - create a renderer endpoint
 * @crtc_id: DRM object ID of the controlled CRTC
 * @connector_id: DRM object ID of the controlled connector
 * @files: pointer to writable struct drm_castkms_renderer_files output storage
 * @flags: zero, or DRM_CASTKMS_RENDERER_CREATE_ADMIN
 * @reserved: must be zero
 *
 * With zero flags, the calling DRM file must be the exact current master and
 * hold both display objects. DRM_CASTKMS_RENDERER_CREATE_ADMIN explicitly asks
 * for administrative issuance and requires CAP_SYS_ADMIN in the initial user
 * namespace. It binds to the independently observed current top-level owner
 * interval, not the calling file's DRM master association. A caller which is
 * itself current master must drop that incidental role before administrative
 * issuance. Absence of a distinct current owner, or a target delegated through
 * a DRM lease, returns EBUSY.
 *
 * The renderer descriptor grants no modesetting or capture access.
 * Its operations authorize delegated rendering for this output and bound
 * drm_master identity. While that master is not current, control operations
 * fail with EACCES. Reacquiring the same master reactivates the descriptor,
 * but work and renderer configurations from the previous uninterrupted interval
 * stay invalid.
 * An administratively issued descriptor is instead permanently stale after
 * its bound owner interval ends; the helper must issue a new descriptor.
 *
 * All request fields are input. Success returns zero after copying both output
 * descriptor numbers and installing their files. On failure neither descriptor
 * is installed; output memory may have been partially written and must not be
 * used. No fallible operation remains after descriptor installation.
 */
struct drm_castkms_create_renderer {
	__u32 crtc_id;
	__u32 connector_id;
	__u64 files;
	__u32 flags;
	__u32 reserved[3];
};

/*
 * Each renderer file owns one immutable configuration and at most one published backend
 * per uninterrupted master interval.
 * The file identifies its configuration; constraints_id identifies the generic
 * constraints entry. Configuration works with disabled video and changes no KMS state.
 * Replacement within an interval uses an independent renderer file. After the
 * bound master is reacquired, a drained endpoint becomes empty and may configure
 * a fresh generation. Publication requires a runnable worker, completed
 * private preparation and registered private images.
 * Only ordinary atomic CONSTRAINTS_ID selection changes the accepted backend.
 *
 * No renderer ioctl returns EAGAIN for readiness. ENODATA means missing
 * required storage or changed job; EBUSY means retry is caller-driven.
 * poll prompts job acquisition, never carries descriptors or proves GPU work
 * complete. Readability does not reserve a job or a particular private image.
 * Withdrawal reports POLLHUP|POLLERR but leaves release/cleanup operations usable.
 */
#define DRM_CASTKMS_RENDERER_STATE_EMPTY 0
#define DRM_CASTKMS_RENDERER_STATE_CONFIGURED 1
#define DRM_CASTKMS_RENDERER_STATE_PUBLISHING 2
#define DRM_CASTKMS_RENDERER_STATE_PUBLISHED 3
#define DRM_CASTKMS_RENDERER_STATE_WITHDRAWN 4

/*
 * Advisory endpoint state under live issuer authority. PUBLISHED means a ready
 * backend was listed, not that KMS selected it. constraints_id is zero until
 * publication, and retained after withdrawal. Reserved output is zero.
 * Query reconciles a lost successful publication reply without creating another
 * backend. Revoked issuer authority returns an error; cleanup remains available.
 */
struct drm_castkms_renderer_query {
	__u32 version;
	__u32 state;
	__u64 constraints_id;
	__u64 reserved[2];
};

/*
 * Configure exactly one immutable whole-state declaration on an empty endpoint.
 * constraints points to constraints_size bytes. width/height are the exact
 * private-pool target within the declared output bounds; they need not match
 * the current mode. Flags/reserved must be zero. Success changes only the
 * configuration.
 * A declaration without a usable allocation intersection with the output's
 * plane topology is EOPNOTSUPP. Failure leaves an empty endpoint retryable; a
 * second declaration is EALREADY.
 */
struct drm_castkms_renderer_configure {
	__u64 constraints;
	__u32 constraints_size;
	__u32 flags;
	__u32 width;
	__u32 height;
	__u64 reserved[3];
};

/*
 * result points to drm_castkms_renderer_publish_result. Flags and reserved must
 * be zero. ready_fence_fd is an already-materialized sync_file
 * covering all private preparation and coherency work, or -1 when that work
 * has already completed. The complete result is copied before native listing;
 * any failure leaves no new selectable backend and copied output must be ignored.
 * Success publishes the configuration for the current master interval; repeating
 * publication in that interval is EALREADY. QUERY returns its identity without
 * publishing again. A pending readiness fence returns EBUSY; a failed fence
 * returns EREMOTEIO, with native status retained on the sync_file. Publication
 * never selects a backend or acknowledges a modeset.
 */
struct drm_castkms_renderer_publish {
	__u64 result;
	__s32 ready_fence_fd;
	__u32 flags;
	__u64 reserved[2];
};

struct drm_castkms_renderer_publish_result {
	__u64 constraints_id;
	__u64 reserved[3];
};

/*
 * Idempotently stop selection and new admission for the endpoint's published
 * backend. Flags/reserved must be zero. Accepted state remains retained, and
 * outstanding reads still require RELEASE_JOB. Unpublished configurations return
 * ENODATA. Withdrawal neither restores default KMS state nor completes native
 * accesses. Closing the renderer file instead ends the reporting channel.
 */
struct drm_castkms_renderer_withdraw {
	__u32 flags;
	__u32 reserved[3];
};

/**
 * struct drm_castkms_renderer_memory_plane - one source image memory plane
 * @dma_buf_fd: returned close-on-exec DMA-BUF descriptor
 * @pitch: byte stride for this plane
 * @offset: byte offset to this plane in the DMA-BUF
 * @reserved: returned as zero
 */
struct drm_castkms_renderer_memory_plane {
	__s32 dma_buf_fd;
	__u32 pitch;
	__u32 offset;
	__u32 reserved;
};


/**
 * struct drm_castkms_renderer_release_job - resolve a source-to-private job
 * @job_id: job returned by RENDERER_ACQUIRE_JOB
 * @release_fence_fd: sync_file descriptor for SUBMITTED, otherwise -1
 * @kind: one DRM_CASTKMS_RENDERER_RELEASE_* value
 * @flags: must be zero
 * @reserved: must be zero
 *
 * NO_ACCESS promises neither source nor private-image access occurred. CPU_DONE
 * promises all CPU access and coherency operations ended. SUBMITTED transfers
 * a native fence covering every submitted source read and private-image write
 * and promises no later submission under this job. Repeating the accepted
 * release for the latest job succeeds. NO_ACCESS produces no private image
 * and leaves the accepted state eligible for another acquisition under a new job ID.
 * That retry still requires current authority and open source-read admission;
 * it cannot reopen a source sealed or held by display preparation.
 */
struct drm_castkms_renderer_release_job {
	__u64 job_id;
	__s32 release_fence_fd;
	__u32 kind;
	__u32 flags;
	__u32 reserved[3];
};

/**
 * struct drm_castkms_renderer_acquire_output - claim recipient output for a private image
 * @result: pointer to writable struct drm_castkms_renderer_output
 * @image_id: completed renderer-private image to copy from
 * @flags: must be zero
 * @reserved: must be zero
 * @padding: must be zero
 *
 * Claims one ready capture destination for an independent private-to-recipient
 * stage. The private image must have completed RELEASE_JOB successfully.
 * This operation acquires no compositor source read. ENODATA means no recipient
 * is ready for this image; EBUSY means another output claim is outstanding.
 * No descriptor is installed on failure. On success RELEASE_OUTPUT is required.
 */
struct drm_castkms_renderer_acquire_output {
	__u64 result;
	__u64 image_id;
	__u32 flags;
	__u32 reserved;
	__u64 padding;
};

/**
 * struct drm_castkms_renderer_output - exact claimed recipient image
 * @job_id: endpoint-local output job identity
 * @image_id: private source image named by ACQUIRE_OUTPUT
 * @width: visible destination width
 * @height: visible destination height
 * @format: DRM fourcc destination format
 * @memory_plane_count: number of destination planes; one in version 1
 * @modifier: destination DRM format modifier
 * @dma_buf_fd: returned close-on-exec writable DMA-BUF descriptor
 * @pitch: destination row pitch in bytes
 * @offset: destination byte offset
 * @reserved: returned as zero
 */
struct drm_castkms_renderer_output {
	__u64 job_id;
	__u64 image_id;
	__u32 width;
	__u32 height;
	__u32 format;
	__u32 memory_plane_count;
	__u64 modifier;
	__s32 dma_buf_fd;
	__u32 pitch;
	__u64 offset;
	__u64 reserved[2];
};

/**
 * struct drm_castkms_renderer_release_output - resolve private-to-recipient access
 * @job_id: job returned by RENDERER_ACQUIRE_OUTPUT
 * @release_fence_fd: sync_file descriptor for SUBMITTED, otherwise -1
 * @kind: one DRM_CASTKMS_RENDERER_RELEASE_* value
 * @flags: must be zero
 * @reserved: must be zero
 *
 * SUBMITTED transfers a native fence covering every private-image read and
 * recipient write. NO_ACCESS promises neither image was accessed. CPU_DONE
 * includes all coherency work. Source release and output release are independent.
 */
struct drm_castkms_renderer_release_output {
	__u64 job_id;
	__s32 release_fence_fd;
	__u32 kind;
	__u32 flags;
	__u32 reserved[3];
};

/* Complete-job stream, native byte order. All records are eight-byte aligned.
 * ACQUIRE_JOB binds accepted KMS plane state to a registered private image until
 * RELEASE_JOB. target_image_id names renderer-private storage registered on this
 * endpoint. It must not alias any source or recipient allocation. The worker
 * must isolate native source queues and mappings from downstream output waits.
 * A busy private image returns EBUSY without admitting source access. A new
 * acquisition withdraws retained content from the selected image before attempting
 * reuse; it never ends outstanding native access. Failed descriptor publication
 * admits no userspace access and permits retry with the same target_image_id.
 * The result consists of a job header, plane records with their color-op records,
 * then output color-op records. Plane order is back-to-front, with zpos ties in
 * KMS plane creation order. Source rectangles use unsigned 16.16 pixels;
 * signed destination positions permit clipping. Sampling is nearest-neighbor.
 * Planes use premultiplied pixel alpha (opaque for formats without alpha), source
 * over an opaque black background. Plane color precedes blending; output color
 * follows blending. All unused memory-plane records contain fd -1 and zeros.
 * A producer already completed with an error makes acquisition return EREMOTEIO;
 * its native errno is never interpreted as queue readiness. No files or source
 * claim are published on that failure. The failed job is discarded; another
 * acquisition returns ENODATA until new state is accepted.
 * The acquire fence covers all planes and must complete successfully before any
 * source read; -1 denotes no outstanding acquire fence. Retaining ordinary
 * DMA-BUF fds does not authorize reads after RELEASE_JOB.
 * No descriptor is installed on failure, even after a partial metadata copy.
 * Allocate JOB_MAX_BYTES for the result; a smaller capacity may return
 * ENOSPC without consuming the job. Flags and reserved fields must be zero.
 * Empty/unchanged jobs return ENODATA, and an outstanding job returns EBUSY.
 */
#define DRM_CASTKMS_RENDERER_JOB_VERSION 1
#define DRM_CASTKMS_RENDERER_JOB_MAX_BYTES 65536
#define DRM_CASTKMS_RENDERER_JOB_MAX_PLANES 24
#define DRM_CASTKMS_RENDERER_JOB_MAX_COLOR_OPS 16
#define DRM_CASTKMS_RENDERER_PLANE_PRIMARY 0
#define DRM_CASTKMS_RENDERER_PLANE_OVERLAY 1
#define DRM_CASTKMS_RENDERER_PLANE_CURSOR 2
#define DRM_CASTKMS_RENDERER_COLOR_OP_BYPASS 0
#define DRM_CASTKMS_RENDERER_COLOR_OP_SRGB_EOTF 1
#define DRM_CASTKMS_RENDERER_COLOR_OP_SRGB_INVERSE_EOTF 2
#define DRM_CASTKMS_RENDERER_COLOR_OP_MATRIX 3
#define DRM_CASTKMS_RENDERER_COLOR_OP_LUT 4

struct drm_castkms_renderer_acquire_job {
	__u64 result;
	__u64 target_image_id;
	__u32 capacity;
	__u32 flags;
	__u64 reserved;
};

/* Private storage is readable/writable only by the trusted renderer. Registration
 * retains one to four distinct DMA-BUFs, not their native format interpretation.
 * The renderer validates layout and import compatibility against its declared
 * constraints.
 * Registration requires a configuration and its exact private-pool dimensions. A
 * published backend pins the complete registration set; no further registrations
 * may extend it. Registration alone authorizes no display-source access.
 * New names are positive and increasing;
 * rejected registration does not consume a name. Flags/reserved must be zero.
 * Registration maps no pixels and authorizes no source or destination access.
 * Known source, recipient and registered allocation aliases are rejected.
 */
struct drm_castkms_renderer_register_image {
	__u64 image_id;
	__u64 buffers; /* Pointer to num_buffers signed 32-bit DMA-BUF descriptors. */
	__u32 width;
	__u32 height;
	__u32 num_buffers;
	__u32 flags;
	__u64 reserved[2];
};

/* Remove a name, not native work. A published backend pins its registrations. After
 * withdrawal/revocation, publishing or claimed source jobs still return EBUSY.
 * Once released, submitted native work independently retains storage and its
 * accounting. Successful removal is not a reuse or completion signal. Cleanup
 * remains available after issuer revocation until final renderer-file close.
 * Names are never reused within the endpoint.
 */
struct drm_castkms_renderer_unregister_image {
	__u64 image_id;
	__u32 flags;
	__u32 reserved;
};

struct drm_castkms_renderer_job {
	__u32 version;
	__u32 bytes;
	__u64 job_id;
	__u64 constraints_id; /* Exact accepted native entry, not continuing authority. */
	__u64 content_serial;
	__u32 width;
	__u32 height;
	__u32 plane_count;
	__s32 acquire_fence_fd;
	__u32 output_color_op_count;
	__u32 reserved;
};

struct drm_castkms_renderer_plane {
	__u32 bytes; /* Includes following color-op records. */
	__u32 role;
	__u32 zpos;
	__u32 format;
	__u64 modifier;
	__u32 width;
	__u32 height;
	__u32 src_x;
	__u32 src_y;
	__u32 src_w;
	__u32 src_h;
	__s32 crtc_x;
	__s32 crtc_y;
	__u32 crtc_w;
	__u32 crtc_h;
	__u32 color_encoding; /* DRM_CASTKMS_YUV_ENCODING_* value, not a mask. */
	__u32 color_range; /* DRM_CASTKMS_YUV_RANGE_* value, not a mask. */
	__u32 memory_plane_count;
	__u32 color_op_count;
	struct drm_castkms_renderer_memory_plane
		memory_planes[DRM_CASTKMS_RENDERER_MAX_MEMORY_PLANES];
};

/* Each color-op record starts with kind and payload_bytes. Curves and bypass
 * have no payload. MATRIX has twelve u64 S31.32 sign-magnitude coefficients
 * (three rows of four, including offsets); LUT has 1..256 entries containing
 * u16 red, green, blue, zero. LUT input/output range is 0..65535 with linear
 * interpolation. Plane matrices retain signed extended range between steps;
 * curves and the pipeline output clamp to 0..65535. Matrix offsets use the
 * same channel units, not normalized 0..1 units. Output operations are applied
 * in degamma-LUT, matrix, gamma-LUT order, omitting absent operations.
 */
struct drm_castkms_renderer_color_op {
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
 * At most one active stream may capture an attachment. Master loss suspends the
 * retained capability with EAGAIN and discards its tap and queued samples. If
 * the same bound drm_master becomes current again, the descriptor opens a fresh
 * tap; samples from the preceding interval never resume. This allows the new
 * current master to create its own stream in the meantime. Creator close,
 * revocation, detach or device removal remains terminal. An old file never
 * follows a replacement attachment. Capture operates independently of the
 * selected video renderer. Disabling the CRTC suspends delivery without
 * revoking the capability. Files are installed only after the complete result
 * has been copied successfully.
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
 * EAGAIN. Authority suspension also returns EAGAIN; poll remains idle until
 * reacquisition or terminal revocation. poll() reports readable samples or
 * terminal POLLHUP|POLLERR.
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
#define DRM_CASTKMS_CREATE_RENDERER 0x01
#define DRM_CASTKMS_MONITOR_QUERY 0x01
#define DRM_CASTKMS_MONITOR_ATTACH 0x02
#define DRM_CASTKMS_MONITOR_DETACH 0x03
#define DRM_CASTKMS_RENDERER_QUERY 0x00
#define DRM_CASTKMS_RENDERER_CONFIGURE 0x01
#define DRM_CASTKMS_RENDERER_PUBLISH 0x02
#define DRM_CASTKMS_RENDERER_WITHDRAW 0x03
#define DRM_CASTKMS_RENDERER_REGISTER_IMAGE 0x04
#define DRM_CASTKMS_RENDERER_UNREGISTER_IMAGE 0x05
#define DRM_CASTKMS_RENDERER_ACQUIRE_JOB 0x06
#define DRM_CASTKMS_RENDERER_RELEASE_JOB 0x07
#define DRM_CASTKMS_RENDERER_ACQUIRE_OUTPUT 0x08
#define DRM_CASTKMS_RENDERER_RELEASE_OUTPUT 0x09

/* This is an enum so that Rust bindgen resolves the ioctl values. */
enum {
	DRM_IOCTL_CASTKMS_RENDERER_CONFIGURE =
		DRM_IOW(DRM_COMMAND_BASE + DRM_CASTKMS_RENDERER_CONFIGURE,
			struct drm_castkms_renderer_configure),
	DRM_IOCTL_CASTKMS_RENDERER_PUBLISH =
		DRM_IOW(DRM_COMMAND_BASE + DRM_CASTKMS_RENDERER_PUBLISH,
			struct drm_castkms_renderer_publish),
	DRM_IOCTL_CASTKMS_RENDERER_WITHDRAW =
		DRM_IOW(DRM_COMMAND_BASE + DRM_CASTKMS_RENDERER_WITHDRAW,
			struct drm_castkms_renderer_withdraw),
	DRM_IOCTL_CASTKMS_RENDERER_REGISTER_IMAGE =
		DRM_IOW(DRM_COMMAND_BASE + DRM_CASTKMS_RENDERER_REGISTER_IMAGE,
			struct drm_castkms_renderer_register_image),
	DRM_IOCTL_CASTKMS_RENDERER_UNREGISTER_IMAGE =
		DRM_IOW(DRM_COMMAND_BASE + DRM_CASTKMS_RENDERER_UNREGISTER_IMAGE,
			struct drm_castkms_renderer_unregister_image),
	DRM_IOCTL_CASTKMS_RENDERER_ACQUIRE_JOB =
		DRM_IOW(DRM_COMMAND_BASE + DRM_CASTKMS_RENDERER_ACQUIRE_JOB,
			struct drm_castkms_renderer_acquire_job),
	DRM_IOCTL_CASTKMS_CREATE_AUDIO_CAPTURE =
		DRM_IOW(DRM_COMMAND_BASE + DRM_CASTKMS_CREATE_AUDIO_CAPTURE,
			struct drm_castkms_create_audio_capture),
	DRM_IOCTL_CASTKMS_AUDIO_QUERY =
		DRM_IOR(DRM_COMMAND_BASE + DRM_CASTKMS_AUDIO_QUERY,
			struct drm_castkms_audio_query),
	DRM_IOCTL_CASTKMS_CREATE_MONITOR_CONTROL =
		DRM_IOW(DRM_COMMAND_BASE + DRM_CASTKMS_CREATE_MONITOR_CONTROL,
			 struct drm_castkms_create_monitor_control),
	DRM_IOCTL_CASTKMS_CREATE_RENDERER =
		DRM_IOW(DRM_COMMAND_BASE + DRM_CASTKMS_CREATE_RENDERER,
			struct drm_castkms_create_renderer),
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
	DRM_IOCTL_CASTKMS_RENDERER_RELEASE_JOB =
		DRM_IOW(DRM_COMMAND_BASE + DRM_CASTKMS_RENDERER_RELEASE_JOB,
			struct drm_castkms_renderer_release_job),
	DRM_IOCTL_CASTKMS_RENDERER_ACQUIRE_OUTPUT =
		DRM_IOW(DRM_COMMAND_BASE + DRM_CASTKMS_RENDERER_ACQUIRE_OUTPUT,
			struct drm_castkms_renderer_acquire_output),
	DRM_IOCTL_CASTKMS_RENDERER_RELEASE_OUTPUT =
		DRM_IOW(DRM_COMMAND_BASE + DRM_CASTKMS_RENDERER_RELEASE_OUTPUT,
			struct drm_castkms_renderer_release_output),
};

#endif
