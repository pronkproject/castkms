// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/align.h>
#include <linux/build_bug.h>
#include <linux/err.h>
#include <linux/export.h>
#include <linux/overflow.h>
#include <linux/string.h>
#include <drm/drm_constraints.h>
#include <drm/drm_constraints_list.h>
#include <drm/drm_constraints_encoding.h>
#include <drm/drm_constraints_entry.h>
#include <drm/drm_fourcc.h>

/* Explicit padding owns every byte copied from these native stack objects. */
static_assert(sizeof(struct drm_mode_constraints_list) == 64);
static_assert(sizeof(struct drm_mode_constraints) == 40);
static_assert(sizeof(struct drm_mode_constraints_description) == 16);
static_assert(sizeof(struct drm_mode_constraints_record) == 16);
static_assert(sizeof(struct drm_mode_constraints_output_size) == 32);
static_assert(sizeof(struct drm_mode_constraints_plane_format) == 72);
static_assert(sizeof(struct drm_mode_constraints_plane_geometry) == 32);
static_assert(sizeof(struct drm_mode_constraints_property) == 56);
static_assert(sizeof(struct drm_mode_constraints_plane_limit) == 24);
static_assert(offsetof(struct drm_mode_constraints_list, generation) == 8);
static_assert(offsetof(struct drm_mode_constraints_list, reserved) == 48);
static_assert(offsetof(struct drm_mode_constraints, reserved) == 24);
static_assert(offsetof(struct drm_mode_constraints_plane_format, modifier) == 24);
static_assert(offsetof(struct drm_mode_constraints_plane_format, layout_flags) == 48);
static_assert(offsetof(struct drm_mode_constraints_plane_format, max_pitch) == 68);
static_assert(offsetof(struct drm_mode_constraints_property, minimum) == 32);
static_assert(DRM_CONSTRAINTS_MAX_ENTRIES <= DRM_MODE_CONSTRAINTS_MAX_ENTRIES);
static_assert(DRM_CONSTRAINTS_MAX_FORMATS <= DRM_MODE_CONSTRAINTS_MAX_FORMATS);
static_assert(DRM_CONSTRAINTS_MAX_PROPERTIES <= DRM_MODE_CONSTRAINTS_MAX_PROPERTIES);
static_assert(DRM_CONSTRAINTS_MAX_PLANE_LIMITS <= DRM_MODE_CONSTRAINTS_MAX_PLANE_LIMITS);
static_assert(DRM_CONSTRAINTS_MAX_PLANE_GEOMETRIES <=
	      DRM_MODE_CONSTRAINTS_MAX_PLANE_GEOMETRIES);
static_assert(DRM_CONSTRAINTS_MAX_PLANES_PER_LIMIT <=
	      DRM_MODE_CONSTRAINTS_MAX_PLANES_PER_LIMIT);
static_assert(sizeof(struct drm_mode_constraints_list) +
	      1ULL * DRM_CONSTRAINTS_MAX_ENTRIES *
	      (sizeof(struct drm_mode_constraints) +
	       sizeof(struct drm_mode_constraints_description) +
	       sizeof(struct drm_mode_constraints_output_size) +
	       DRM_CONSTRAINTS_MAX_FORMATS * sizeof(struct drm_mode_constraints_plane_format) +
	       DRM_CONSTRAINTS_MAX_PROPERTIES * sizeof(struct drm_mode_constraints_property) +
	       DRM_CONSTRAINTS_MAX_PLANE_GEOMETRIES *
	       sizeof(struct drm_mode_constraints_plane_geometry) +
	       DRM_CONSTRAINTS_MAX_PLANE_LIMITS *
	       ALIGN(sizeof(struct drm_mode_constraints_plane_limit) +
		     DRM_CONSTRAINTS_MAX_PLANES_PER_LIMIT * sizeof(__u32), 8)) <=
	      DRM_MODE_CONSTRAINTS_MAX_BYTES);

static size_t plane_limit_size(const struct drm_constraints_plane_limit *limit)
{
	return ALIGN(sizeof(struct drm_mode_constraints_plane_limit) +
		     size_mul(limit->count, sizeof(__u32)), 8);
}

static size_t description_size(struct drm_constraints_description *description)
{
	const struct drm_constraints_plane_limit *plane_limits;
	unsigned int formats, properties, plane_limit_count, plane_geometry_count, i;
	size_t geometry_bytes, records;

	drm_constraints_description_formats(description, &formats);
	drm_constraints_description_properties(description, &properties);
	records = size_add(size_mul(formats, sizeof(struct drm_mode_constraints_plane_format)),
			   size_mul(properties, sizeof(struct drm_mode_constraints_property)));
	drm_constraints_description_plane_geometries(description, &plane_geometry_count);
	geometry_bytes = size_mul(plane_geometry_count,
				  sizeof(struct drm_mode_constraints_plane_geometry));
	records = size_add(records, geometry_bytes);
	plane_limits = drm_constraints_description_plane_limits(description, &plane_limit_count);
	for (i = 0; i < plane_limit_count; i++)
		records = size_add(records, plane_limit_size(&plane_limits[i]));
	return size_add(sizeof(struct drm_mode_constraints_description) +
			sizeof(struct drm_mode_constraints_output_size), records);
}

static size_t encode_description(struct drm_constraints_description *description,
				 void *buffer, size_t offset)
{
	const struct drm_constraints_size *size = drm_constraints_description_output(description);
	const struct drm_constraints_format *formats;
	const struct drm_constraints_property *properties;
	const struct drm_constraints_plane_geometry *plane_geometries;
	const struct drm_constraints_plane_limit *plane_limits;
	unsigned int format_count, property_count, plane_limit_count, plane_geometry_count, i;
	struct drm_mode_constraints_description header;
	struct drm_mode_constraints_output_size output = {
		.header = {
			.type = DRM_MODE_CONSTRAINTS_RECORD_OUTPUT_SIZE,
			.flags = DRM_MODE_CONSTRAINTS_RECORD_REQUIRED,
			.length = sizeof(output),
		},
		.min_width = size->min_width, .min_height = size->min_height,
		.max_width = size->max_width, .max_height = size->max_height,
	};

	formats = drm_constraints_description_formats(description, &format_count);
	properties = drm_constraints_description_properties(description, &property_count);
	plane_geometries =
		drm_constraints_description_plane_geometries(description, &plane_geometry_count);
	plane_limits = drm_constraints_description_plane_limits(description, &plane_limit_count);
	header = (struct drm_mode_constraints_description) {
		.version = DRM_MODE_CONSTRAINTS_VERSION,
		.length = description_size(description),
		.record_count = 1 + format_count + property_count + plane_geometry_count +
			plane_limit_count,
		.records_offset = offset + sizeof(header),
	};
	memcpy((u8 *)buffer + offset, &header, sizeof(header));
	offset += sizeof(header);
	memcpy((u8 *)buffer + offset, &output, sizeof(output));
	offset += sizeof(output);
	for (i = 0; i < format_count; i++) {
		const struct drm_format_info *info = __drm_format_info(formats[i].format);
		struct drm_mode_constraints_plane_format record = {
			.header = {
				.type = DRM_MODE_CONSTRAINTS_RECORD_PLANE_FORMAT,
				.flags = DRM_MODE_CONSTRAINTS_RECORD_REQUIRED,
				.length = sizeof(record),
			},
			.plane_id = formats[i].plane_id, .format = formats[i].format,
			.modifier = formats[i].modifier,
			.min_width = formats[i].size.min_width,
			.min_height = formats[i].size.min_height,
			.max_width = formats[i].size.max_width,
			.max_height = formats[i].size.max_height,
			.layout_flags = formats[i].flags & DRM_CONSTRAINTS_FORMAT_IMPLICIT ?
					DRM_MODE_CONSTRAINTS_LAYOUT_IMPLICIT : 0,
			.storage_flags =
				(formats[i].storage_flags & DRM_CONSTRAINTS_FORMAT_STORAGE_NATIVE ?
				 DRM_MODE_CONSTRAINTS_FORMAT_STORAGE_NATIVE : 0) |
				(formats[i].storage_flags &
				 DRM_CONSTRAINTS_FORMAT_STORAGE_IMPORTED ?
				 DRM_MODE_CONSTRAINTS_FORMAT_STORAGE_IMPORTED : 0),
			.plane_count = info->num_planes,
			.pitch_alignment = formats[i].pitch_alignment,
			.offset_alignment = formats[i].offset_alignment,
			.max_pitch = formats[i].max_pitch,
		};

		memcpy((u8 *)buffer + offset, &record, sizeof(record));
		offset += sizeof(record);
	}
	for (i = 0; i < property_count; i++) {
		struct drm_mode_constraints_property record = {
			.header = {
				.type = DRM_MODE_CONSTRAINTS_RECORD_PROPERTY,
				.flags = DRM_MODE_CONSTRAINTS_RECORD_REQUIRED,
				.length = sizeof(record),
			},
			.object_id = properties[i].object_id,
			.property_id = properties[i].property_id,
			.type = properties[i].type,
			.applicability_flags =
				properties[i].flags & DRM_CONSTRAINTS_PROPERTY_PLANE_YUV ?
				DRM_MODE_CONSTRAINTS_PROPERTY_PLANE_YUV : 0,
			.minimum = properties[i].minimum, .maximum = properties[i].maximum,
			.mask = properties[i].mask,
		};

		memcpy((u8 *)buffer + offset, &record, sizeof(record));
		offset += sizeof(record);
	}
	for (i = 0; i < plane_geometry_count; i++) {
		struct drm_mode_constraints_plane_geometry record = {
			.header = {
				.type = DRM_MODE_CONSTRAINTS_RECORD_PLANE_GEOMETRY,
				.flags = DRM_MODE_CONSTRAINTS_RECORD_REQUIRED,
				.length = sizeof(record),
			},
			.plane_id = plane_geometries[i].plane_id,
			.flags =
				(plane_geometries[i].flags & DRM_CONSTRAINTS_GEOMETRY_CROP ?
				 DRM_MODE_CONSTRAINTS_GEOMETRY_CROP : 0) |
				(plane_geometries[i].flags &
				 DRM_CONSTRAINTS_GEOMETRY_FRACTIONAL_SOURCE ?
				 DRM_MODE_CONSTRAINTS_GEOMETRY_FRACTIONAL_SOURCE : 0) |
				(plane_geometries[i].flags & DRM_CONSTRAINTS_GEOMETRY_POSITION ?
				 DRM_MODE_CONSTRAINTS_GEOMETRY_POSITION : 0),
			.min_scale = plane_geometries[i].min_scale,
			.max_scale = plane_geometries[i].max_scale,
		};

		memcpy((u8 *)buffer + offset, &record, sizeof(record));
		offset += sizeof(record);
	}
	for (i = 0; i < plane_limit_count; i++) {
		size_t length = plane_limit_size(&plane_limits[i]);
		struct drm_mode_constraints_plane_limit record = {
			.header = {
				.type = DRM_MODE_CONSTRAINTS_RECORD_PLANE_LIMIT,
				.flags = DRM_MODE_CONSTRAINTS_RECORD_REQUIRED,
				.length = length,
			},
			.max_active = plane_limits[i].max_active,
			.count_planes = plane_limits[i].count,
		};

		memcpy((u8 *)buffer + offset, &record, sizeof(record));
		memcpy((u8 *)buffer + offset + sizeof(record), plane_limits[i].plane_ids,
		       sizeof(__u32) * plane_limits[i].count);
		memset((u8 *)buffer + offset + sizeof(record) +
		       sizeof(__u32) * plane_limits[i].count, 0,
		       length - sizeof(record) - sizeof(__u32) * plane_limits[i].count);
		offset += length;
	}
	return offset;
}

int drm_constraints_snapshot_encode(const struct drm_constraints_snapshot *snapshot,
				     void *buffer, size_t capacity, size_t *required)
{
	const struct drm_constraints_snapshot_info *info;
	const struct drm_constraints_listing *entries;
	struct drm_mode_constraints_list header;
	size_t length, offset;
	unsigned int i;

	if (!snapshot || !required || (!buffer && capacity))
		return -EINVAL;
	info = drm_constraints_snapshot_info(snapshot);
	entries = drm_constraints_snapshot_entries(snapshot);
	offset = size_add(sizeof(header), size_mul(info->count,
						  sizeof(struct drm_mode_constraints)));
	length = offset;
	for (i = 0; i < info->count; i++)
		length = size_add(length,
			description_size(drm_constraints_entry_description(entries[i].entry)));
	if (length > DRM_MODE_CONSTRAINTS_MAX_BYTES)
		return -E2BIG;
	*required = length;
	if (!buffer)
		return 0;
	if (capacity < length)
		return -ENOSPC;
	header = (struct drm_mode_constraints_list) {
		.version = DRM_MODE_CONSTRAINTS_VERSION, .length = length,
		.generation = info->generation, .selected_id = info->selected_id,
		.suggested_id = info->suggested_id, .count_entries = info->count,
		.entries_offset = sizeof(header),
		.entry_size = sizeof(struct drm_mode_constraints),
	};
	memcpy(buffer, &header, sizeof(header));
	for (i = 0; i < info->count; i++) {
		struct drm_constraints_description *description =
			drm_constraints_entry_description(entries[i].entry);
		struct drm_mode_constraints entry = {
			.id = drm_constraints_entry_id(entries[i].entry),
			.flags = entries[i].selectable ? DRM_MODE_CONSTRAINTS_SELECTABLE : 0,
			.description_offset = offset,
			.description_length = description_size(description),
		};

		memcpy((u8 *)buffer + sizeof(header) + i * sizeof(entry), &entry, sizeof(entry));
		offset = encode_description(description, buffer, offset);
	}
	return 0;
}
EXPORT_SYMBOL_GPL(drm_constraints_snapshot_encode);
