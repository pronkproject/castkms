/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_CONSTRAINTS_ENCODING_H__
#define __DRM_CONSTRAINTS_ENCODING_H__

#include <linux/types.h>

struct drm_constraints_snapshot;

/* Kernel prototype encoding, not an installed UAPI or allocated ioctl. */
#define DRM_CONSTRAINTS_ENCODING_VERSION 1
#define DRM_CONSTRAINTS_ENCODING_MAX_SIZE (16U * 1024U * 1024U)
#define DRM_CONSTRAINTS_ENCODED_SELECTABLE (1U << 0)
#define DRM_CONSTRAINTS_RECORD_REQUIRED (1U << 0)
#define DRM_CONSTRAINTS_RECORD_OUTPUT 1
#define DRM_CONSTRAINTS_RECORD_FORMAT 2
#define DRM_CONSTRAINTS_RECORD_PROPERTY 3

/*
 * Native-endian, identical layouts on 32/64-bit kernels; every offset is
 * relative to the beginning of the snapshot. Padding and reserves are zero.
 */
struct drm_constraints_encoded_list {
	u32 version;
	u32 length;
	__aligned_u64 generation;
	__aligned_u64 selected_id;
	__aligned_u64 suggested_id;
	u32 count_entries;
	u32 entries_offset;
	u32 entry_size;
	u32 pad;
	__aligned_u64 reserved[2];
};

struct drm_constraints_encoded_entry {
	__aligned_u64 id;
	u32 flags;
	u32 description_offset;
	u32 description_length;
	u32 pad;
	__aligned_u64 reserved[2];
};

struct drm_constraints_encoded_description {
	u32 version;
	u32 length;
	u32 record_count;
	u32 records_offset;
};

struct drm_constraints_encoded_record {
	u32 type;
	u32 flags;
	u32 length;
	u32 pad;
};

struct drm_constraints_encoded_output {
	struct drm_constraints_encoded_record header;
	u32 min_width;
	u32 min_height;
	u32 max_width;
	u32 max_height;
};

struct drm_constraints_encoded_format {
	struct drm_constraints_encoded_record header;
	u32 plane_id;
	u32 format;
	__aligned_u64 modifier;
	u32 min_width;
	u32 min_height;
	u32 max_width;
	u32 max_height;
};

struct drm_constraints_encoded_property {
	struct drm_constraints_encoded_record header;
	u32 object_id;
	u32 property_id;
	u32 type;
	u32 pad;
	__aligned_u64 minimum;
	__aligned_u64 maximum;
	__aligned_u64 mask;
};

/*
 * Encode one retained immutable snapshot into a kernel buffer. NULL/zero is
 * size discovery. ENOSPC reports required bytes without modifying any payload;
 * other errors leave required unchanged. No alignment of buffer is required;
 * required and the retained snapshot must not overlap the output buffer.
 * Success writes exactly required bytes, with no references or authority in
 * the encoding. Snapshot ownership and expected-generation checking belong to
 * the list; copyout, client authority and notification delivery are separate.
 * Readers must reject unsupported versions and entries with unknown required
 * records. Per-plane format records are alternatives; scalar rules all apply.
 */
int drm_constraints_snapshot_encode(const struct drm_constraints_snapshot *snapshot,
				     void *buffer, size_t capacity, size_t *required);

#endif
