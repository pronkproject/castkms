// SPDX-License-Identifier: GPL-2.0 OR MIT

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
static_assert(sizeof(struct drm_mode_constraints_property) == 56);
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
static_assert(sizeof(struct drm_mode_constraints_list) +
	      1ULL * DRM_CONSTRAINTS_MAX_ENTRIES *
	      (sizeof(struct drm_mode_constraints) +
	       sizeof(struct drm_mode_constraints_description) +
	       sizeof(struct drm_mode_constraints_output_size) +
	       DRM_CONSTRAINTS_MAX_FORMATS * sizeof(struct drm_mode_constraints_plane_format) +
	       DRM_CONSTRAINTS_MAX_PROPERTIES * sizeof(struct drm_mode_constraints_property)) <=
	      DRM_MODE_CONSTRAINTS_MAX_BYTES);

static size_t description_size(struct drm_constraints_description *description)
{
	unsigned int formats, properties;
	size_t records;

	drm_constraints_description_formats(description, &formats);
	drm_constraints_description_properties(description, &properties);
	records = size_add(size_mul(formats, sizeof(struct drm_mode_constraints_plane_format)),
			   size_mul(properties, sizeof(struct drm_mode_constraints_property)));
	return size_add(sizeof(struct drm_mode_constraints_description) +
			sizeof(struct drm_mode_constraints_output_size), records);
}

static size_t encode_description(struct drm_constraints_description *description,
				 void *buffer, size_t offset)
{
	const struct drm_constraints_size *size = drm_constraints_description_output(description);
	const struct drm_constraints_format *formats;
	const struct drm_constraints_property *properties;
	unsigned int format_count, property_count, i;
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
	header = (struct drm_mode_constraints_description) {
		.version = DRM_MODE_CONSTRAINTS_VERSION,
		.length = description_size(description),
		.record_count = 1 + format_count + property_count,
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
			.minimum = properties[i].minimum, .maximum = properties[i].maximum,
			.mask = properties[i].mask,
		};

		memcpy((u8 *)buffer + offset, &record, sizeof(record));
		offset += sizeof(record);
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
