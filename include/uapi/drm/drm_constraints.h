/* SPDX-License-Identifier: MIT */
#ifndef _UAPI_DRM_CONSTRAINTS_H_
#define _UAPI_DRM_CONSTRAINTS_H_

#include "drm.h"

/* Experimental constraints description interface. */
/*
 * Set the client capability to 1 after enabling DRM_CLIENT_CAP_ATOMIC to opt
 * into KMS constraints and subscribe to list-change events. Neither capability
 * grants modesetting authority. Query each output at setup and resume.
 *
 * Set it to 0 to stop new notifications. Previously queued records remain
 * readable. A current master cannot disable it while a visible participating
 * output has nondefault accepted constraints: restore those defaults first,
 * or relinquish modesetting ownership. Such a disable returns EBUSY.
 * Disable this capability before disabling DRM_CLIENT_CAP_ATOMIC. Values
 * other than 0 or 1 return EINVAL; unsupported devices return EOPNOTSUPP.
 */
#define DRM_CLIENT_CAP_KMS_CONSTRAINTS 9
#define DRM_CONSTRAINTS_ID_PROPERTY "CONSTRAINTS_ID"

/*
 * CONSTRAINTS_ID is persistent atomic CRTC state on participating outputs.
 * After client opt-in, select a listed positive ID alongside its compatible
 * framebuffer, geometry, color and synchronization state. Omission retains
 * the accepted binding; repeating it requests no transition. Zero is invalid.
 * A changed ID requires DRM_MODE_ATOMIC_ALLOW_MODESET. Readback describes
 * accepted state, not presentation or native execution completion.
 *
 * TEST_ONLY reserves nothing. Withdrawn or stale targets fail with ESTALE at
 * real acceptance; an incompatible scene gets the ordinary validation error.
 * Outstanding leases retain their output's fixed default contract; selecting
 * other constraints returns EBUSY. An accepted binding remains retained through
 * retirement, including terminal backend failure. Disabling a failed output
 * may retain its binding but cannot select another one through a closed list.
 */

#define DRM_MODE_CONSTRAINTS_VERSION 1
#define DRM_MODE_CONSTRAINTS_MAX_BYTES (16U * 1024U * 1024U)
#define DRM_MODE_CONSTRAINTS_MAX_ENTRIES 64U
#define DRM_MODE_CONSTRAINTS_MAX_FORMATS 4096U
#define DRM_MODE_CONSTRAINTS_MAX_PROPERTIES 64U

#define DRM_MODE_CONSTRAINTS_SELECTABLE (1U << 0)
#define DRM_MODE_CONSTRAINTS_RECORD_REQUIRED (1U << 0)
#define DRM_MODE_CONSTRAINTS_RECORD_OUTPUT_SIZE 1U
#define DRM_MODE_CONSTRAINTS_RECORD_PLANE_FORMAT 2U
#define DRM_MODE_CONSTRAINTS_RECORD_PROPERTY 3U
#define DRM_MODE_CONSTRAINTS_LAYOUT_IMPLICIT (1U << 0)

#define DRM_EVENT_KMS_CONSTRAINTS_LIST_CHANGED 0x04
#define DRM_KMS_CONSTRAINTS_LIST_CLOSED (1U << 0)

/**
 * struct drm_event_kms_constraints_list_changed - Advisory output list change
 * @base: Standard DRM event header; type DRM_EVENT_KMS_CONSTRAINTS_LIST_CHANGED.
 * @crtc_id: CRTC whose constraints list changed.
 * @flags: DRM_KMS_CONSTRAINTS_LIST_CLOSED, or zero.
 * @generation: Observed nonzero list generation, or zero when CLOSED is set.
 * @reserved: Zero.
 *
 * Prompts a fresh DRM_IOCTL_MODE_LIST_CONSTRAINTS query. Notifications coalesce:
 * clients need not observe every intermediate generation. A queued record is
 * immutable, so a newer change may follow it after consumption. Full event
 * queues retain pending changes until capacity is returned; failed reads do
 * not consume an event. CLOSED is terminal: further queries return ESTALE.
 *
 * This event grants no authority, selects no entry and signals neither display
 * completion nor native GPU completion. Query on initial setup and resume even
 * when no event has arrived. Delivery requires explicit client subscription;
 * read-only listing does not subscribe a file. Userspace event dispatch must
 * expose this record rather than silently discarding an unknown event type.
 */
struct drm_event_kms_constraints_list_changed {
	struct drm_event base;
	__u32 crtc_id;
	__u32 flags;
	__aligned_u64 generation;
	__aligned_u64 reserved;
};

/**
 * struct drm_mode_list_constraints - Copy one output's constraints snapshot
 * @crtc_id: Output CRTC object ID.
 * @flags: Must be zero.
 * @generation: Expected generation or zero; returned snapshot generation.
 * @data: Userspace address for the snapshot, or zero for size discovery.
 * @size: Buffer capacity on input; required snapshot bytes on output.
 * @pad: Must be zero.
 * @reserved: Must be zero.
 *
 * Set both data and size to zero for size discovery. Otherwise both must be
 * nonzero. A nonzero expected generation must match or the operation returns
 * ESTALE. Success and ENOSPC report generation and required size. ENOSPC does
 * not modify the snapshot buffer. Other errors provide no usable snapshot or
 * output metadata; discard any partial payload on EFAULT. Only the required
 * bytes are copied, irrespective of a larger advertised capacity.
 *
 * Neither size discovery nor a successful fetch reserves an entry for later
 * selection. Identifiers and descriptions grant no modesetting or pixel access.
 */
struct drm_mode_list_constraints {
	__u32 crtc_id;
	__u32 flags;
	__aligned_u64 generation;
	__aligned_u64 data;
	__u32 size;
	__u32 pad;
	__aligned_u64 reserved[2];
};

/* Read-only discovery for the current modesetting master, within its CRTC
 * visibility. Discovery does not opt a file into changing constraints or
 * receiving change notifications. Unsupported outputs return EOPNOTSUPP;
 * unknown or inaccessible CRTC IDs return ENOENT. This operation never returns
 * EAGAIN and may be called through libdrm's normal drmIoctl() wrapper.
 */
#define DRM_IOCTL_MODE_LIST_CONSTRAINTS DRM_IOWR(0xD5, struct drm_mode_list_constraints)

/**
 * struct drm_mode_constraints_list - One consistent constraints snapshot
 * @version: DRM_MODE_CONSTRAINTS_VERSION.
 * @length: Total snapshot bytes, at most DRM_MODE_CONSTRAINTS_MAX_BYTES.
 * @generation: Nonzero list generation, not a presentation sequence.
 * @selected_id: Accepted selection, not confirmation of completed presentation.
 * @suggested_id: Advisory selectable target, or zero for no suggestion.
 * @count_entries: From one through DRM_MODE_CONSTRAINTS_MAX_ENTRIES.
 * @entries_offset: Byte offset of the struct drm_mode_constraints array.
 * @entry_size: Byte stride, sizeof(struct drm_mode_constraints) for version 1.
 * @pad: Zero.
 * @reserved: Zero.
 *
 * All integers use native byte order. All offsets in the snapshot, including
 * offsets nested inside descriptions, are relative to its beginning. Structures
 * have the same layout for 32-bit and 64-bit callers. Offsets and record lengths
 * are eight-byte aligned. Validate version, bounds, strides and arithmetic before
 * accessing arrays or records. Unknown versions are unsupported, not unrestricted.
 *
 * A generation changes when entries, availability, selection or suggestion
 * change, not for every frame. A snapshot does not reserve later availability.
 * Descriptions guide allocation; the driver still validates complete atomic
 * states, combinations and shared resources. No identifier or description
 * grants modesetting, pixel access or authority over a renderer.
 */
struct drm_mode_constraints_list {
	__u32 version;
	__u32 length;
	__aligned_u64 generation;
	__aligned_u64 selected_id;
	__aligned_u64 suggested_id;
	__u32 count_entries;
	__u32 entries_offset;
	__u32 entry_size;
	__u32 pad;
	__aligned_u64 reserved[2];
};

/**
 * struct drm_mode_constraints - Immutable meaning with snapshot availability
 * @id: Positive constraints identity, never reused during the device lifetime.
 * @flags: DRM_MODE_CONSTRAINTS_SELECTABLE when selectable at snapshot creation.
 * @description_offset: Offset of struct drm_mode_constraints_description.
 * @description_length: Bytes occupied by that description and its records.
 * @pad: Zero.
 * @reserved: Zero.
 *
 * Availability may change without changing the meaning of @id. The selected
 * entry remains described even if withdrawn. Retaining an accepted binding is
 * not permission to select a withdrawn entry from a different binding. A worker
 * failure after acceptance is not an instruction to reinterpret its buffers
 * under another entry. Unknown entry flags make the entry unsupported.
 */
struct drm_mode_constraints {
	__aligned_u64 id;
	__u32 flags;
	__u32 description_offset;
	__u32 description_length;
	__u32 pad;
	__aligned_u64 reserved[2];
};

/**
 * struct drm_mode_constraints_description - Length-delimited allocation rules
 * @version: DRM_MODE_CONSTRAINTS_VERSION.
 * @length: Description header and records, matching the entry's length.
 * @record_count: Number of consecutive length-delimited records.
 * @records_offset: Offset of the first struct drm_mode_constraints_record.
 *
 * Exactly one output-size record describes mode dimensions. Plane-format
 * records describe alternative framebuffer allocations for each named plane;
 * a plane with no format records must not be used. There are at most
 * DRM_MODE_CONSTRAINTS_MAX_FORMATS format records and
 * DRM_MODE_CONSTRAINTS_MAX_PROPERTIES property records per description.
 * Property records all apply, but only to enabled CRTCs and used planes.
 * Absence of a property record preserves ordinary KMS property semantics.
 * Geometry relationships, blob contents and other full-scene restrictions
 * remain subject to atomic validation; absence of a record does not bypass it.
 */
struct drm_mode_constraints_description {
	__u32 version;
	__u32 length;
	__u32 record_count;
	__u32 records_offset;
};

/**
 * struct drm_mode_constraints_record - Header common to every description rule
 * @type: One of the DRM_MODE_CONSTRAINTS_RECORD_* type constants.
 * @flags: DRM_MODE_CONSTRAINTS_RECORD_REQUIRED for rules necessary to use an entry.
 * @length: Complete record bytes, at least the header size and eight-byte aligned.
 * @pad: Zero.
 *
 * Skip unknown optional records using @length. An unknown required record,
 * unknown record flag or malformed length makes the entry unsupported. Known
 * version-1 record types have the exact sizes defined below. Records must fit
 * within both the enclosing description and snapshot, without overlap with
 * their headers or the entry array. Padding and reserved fields are zero.
 */
struct drm_mode_constraints_record {
	__u32 type;
	__u32 flags;
	__u32 length;
	__u32 pad;
};

/**
 * struct drm_mode_constraints_output_size - Inclusive mode dimensions
 * @header: OUTPUT_SIZE record header.
 * @min_width: Nonzero minimum horizontal mode dimension in pixels.
 * @min_height: Nonzero minimum vertical mode dimension in pixels.
 * @max_width: Maximum horizontal mode dimension, at least @min_width.
 * @max_height: Maximum vertical mode dimension, at least @min_height.
 *
 * Equal minimum and maximum on an axis require that exact dimension. The
 * selected mode must independently satisfy ordinary KMS timing validation.
 */
struct drm_mode_constraints_output_size {
	struct drm_mode_constraints_record header;
	__u32 min_width;
	__u32 min_height;
	__u32 max_width;
	__u32 max_height;
};

/**
 * struct drm_mode_constraints_plane_format - One plane's allocation alternative
 * @header: PLANE_FORMAT record header.
 * @plane_id: Existing plane object ID on the queried DRM device.
 * @format: DRM fourcc.
 * @modifier: Explicit DRM modifier, or zero with LAYOUT_IMPLICIT.
 * @min_width: Nonzero minimum framebuffer width in pixels.
 * @min_height: Nonzero minimum framebuffer height in pixels.
 * @max_width: Maximum framebuffer width, at least @min_width.
 * @max_height: Maximum framebuffer height, at least @min_height.
 * @layout_flags: Zero or DRM_MODE_CONSTRAINTS_LAYOUT_IMPLICIT.
 * @pad: Zero.
 *
 * Dimensions describe framebuffer allocation, not the fractional source
 * rectangle or scaled destination. LAYOUT_IMPLICIT means framebuffer creation
 * without DRM_MODE_FB_MODIFIERS; it does not promise linear storage. Explicit
 * LINEAR has modifier zero and layout_flags zero. Unknown layout flags make
 * the entry unsupported. Tuples of plane, format, modifier and layout flags
 * are unique within a description.
 *
 * These records do not rewrite IN_FORMATS. Target framebuffer construction
 * precedes selecting the target entry; constructing a framebuffer does not
 * authorize displaying it under the currently selected constraints.
 */
struct drm_mode_constraints_plane_format {
	struct drm_mode_constraints_record header;
	__u32 plane_id;
	__u32 format;
	__aligned_u64 modifier;
	__u32 min_width;
	__u32 min_height;
	__u32 max_width;
	__u32 max_height;
	__u32 layout_flags;
	__u32 pad;
};

/**
 * struct drm_mode_constraints_property - One standard scalar property restriction
 * @header: PROPERTY record header.
 * @object_id: Existing CRTC or plane ID on the queried device.
 * @property_id: Standard scalar scene property attached to @object_id.
 * @type: DRM_MODE_PROP_RANGE, SIGNED_RANGE, ENUM or BITMASK; no other bits.
 * @pad: Zero.
 * @minimum: Inclusive range minimum; zero for enum or bitmask.
 * @maximum: Inclusive range maximum; zero for enum or bitmask.
 * @mask: Permitted enum values or bitmask bits; zero for range types.
 *
 * Signed ranges use DRM's two's-complement 64-bit representation. Enum values
 * 0 through 63 correspond to their mask bits; an enum mask is nonempty. A
 * bitmask rule may allow only zero. Object/property pairs are unique. Type
 * and permitted values respect the attached property's ordinary semantics.
 * Blob data, object references, immutable properties and request-only inputs
 * are not scalar rules. Unknown types make the entry unsupported.
 */
struct drm_mode_constraints_property {
	struct drm_mode_constraints_record header;
	__u32 object_id;
	__u32 property_id;
	__u32 type;
	__u32 pad;
	__aligned_u64 minimum;
	__aligned_u64 maximum;
	__aligned_u64 mask;
};

#endif /* _UAPI_DRM_CONSTRAINTS_H_ */
