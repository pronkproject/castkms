// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/align.h>
#include <linux/module.h>
#include <linux/slab.h>
#include <drm/drm_constraints.h>
#include <drm/drm_constraints_list.h>
#include <drm/drm_constraints_encoding.h>
#include <drm/drm_constraints_entry.h>
#include <drm/drm_fourcc.h>
#include <drm/drm_mode.h>
#include <kunit/test.h>

struct encoding_fixture {
	struct drm_constraints_domain *domain;
	struct drm_constraints_description *description;
	struct drm_constraints_entry *initial;
	struct drm_constraints_list *list;
};

static void release_backend(void *data) {}
static const struct drm_constraints_entry_ops ops = {
	.owner = THIS_MODULE, .release = release_backend,
};
static void put_domain(void *data) { drm_constraints_domain_put(data); }
static void put_description(void *data) { drm_constraints_description_put(data); }
static void put_entry(void *data) { drm_constraints_entry_put(data); }
static void put_list(void *data) { drm_constraints_list_put(data); }
static void put_snapshot(void *data) { drm_constraints_snapshot_put(data); }
static void free_buffer(void *data) { kvfree(data); }

static struct encoding_fixture *new_fixture_with_layout(struct kunit *test, bool implicit)
{
	const struct drm_constraints_size output = { 640, 360, 1920, 1080 };
	const struct drm_constraints_format format = {
		.plane_id = 7, .format = DRM_FORMAT_XRGB8888,
		.modifier = implicit ? 0 : I915_FORMAT_MOD_X_TILED,
		.size = { 64, 32, 3840, 2160 },
		.flags = implicit ? DRM_CONSTRAINTS_FORMAT_IMPLICIT : 0,
		.storage_flags = DRM_CONSTRAINTS_FORMAT_STORAGE_IMPORTED,
		.width_alignment = 64,
		.height_alignment = 4,
		.pitch_alignment = 256,
		.offset_alignment = 4096,
		.min_pitch = 1024,
		.max_pitch = 65536,
	};
	struct drm_constraints_property property;
	u32 plane_ids[] = { 7, 8, 9 };
	const struct drm_constraints_plane_limit plane_limit = {
		.max_active = 2, .count = ARRAY_SIZE(plane_ids), .plane_ids = plane_ids,
	};
	const struct drm_constraints_plane_geometry geometry = {
		.plane_id = 7,
		.flags = DRM_CONSTRAINTS_GEOMETRY_CROP |
			 DRM_CONSTRAINTS_GEOMETRY_FRACTIONAL_SOURCE,
		.min_scale = 1 << 15,
		.max_scale = 1 << 17,
	};
	struct encoding_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);

	KUNIT_ASSERT_NOT_NULL(test, f);
	/* Native record padding must never leak into the encoded representation. */
	memset(&property, 0xa5, sizeof(property));
	property.object_id = 7;
	property.property_id = 11;
	property.type = DRM_MODE_PROP_SIGNED_RANGE;
	property.flags = DRM_CONSTRAINTS_PROPERTY_PLANE_YUV;
	property.minimum = (u64)-10;
	property.maximum = 20;
	property.mask = 0;
	f->domain = drm_constraints_domain_create(DRM_CONSTRAINTS_MAX_ENTRIES);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->domain);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_domain, f->domain), 0);
	f->description = drm_constraints_description_create_with_geometry(
		&output, &format, 1, &property, 1, &plane_limit, 1, &geometry, 1);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->description);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_description, f->description), 0);
	f->initial = drm_constraints_entry_create(f->domain, 19, f->description, &ops, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->initial);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_entry, f->initial), 0);
	f->list = drm_constraints_list_create(f->domain, f->initial, DRM_CONSTRAINTS_MAX_ENTRIES);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->list);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_list, f->list), 0);
	return f;
}

static struct encoding_fixture *new_fixture(struct kunit *test)
{
	return new_fixture_with_layout(test, false);
}

static struct drm_constraints_snapshot *snapshot(struct kunit *test,
						 struct drm_constraints_list *list)
{
	struct drm_constraints_snapshot *snapshot = drm_constraints_list_snapshot(list, 0);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, snapshot);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_snapshot, snapshot), 0);
	return snapshot;
}

static void check_encoded_layout(struct kunit *test, bool implicit)
{
	enum { expected_bytes = sizeof(struct drm_mode_constraints_list) +
				sizeof(struct drm_mode_constraints) +
				sizeof(struct drm_mode_constraints_description) +
				sizeof(struct drm_mode_constraints_output_size) +
				sizeof(struct drm_mode_constraints_plane_format) +
				sizeof(struct drm_mode_constraints_property) +
				sizeof(struct drm_mode_constraints_plane_geometry) +
				ALIGN(sizeof(struct drm_mode_constraints_plane_limit) +
				      3 * sizeof(__u32), 8) };
	struct encoding_fixture *f = new_fixture_with_layout(test, implicit);
	struct drm_constraints_snapshot *view = snapshot(test, f->list);
	struct drm_mode_constraints_list *header;
	struct drm_mode_constraints *entry;
	struct drm_mode_constraints_description *description;
	struct drm_mode_constraints_output_size *output;
	struct drm_mode_constraints_plane_format *format;
	struct drm_mode_constraints_property *property;
	struct drm_mode_constraints_plane_geometry *geometry;
	struct drm_mode_constraints_plane_limit *plane_limit;
	size_t required = 0;
	u8 *buffer;

	KUNIT_ASSERT_EQ(test, drm_constraints_snapshot_encode(view, NULL, 0, &required), 0);
	KUNIT_ASSERT_EQ(test, required, expected_bytes);
	buffer = kunit_kmalloc(test, required, GFP_KERNEL);
	KUNIT_ASSERT_NOT_NULL(test, buffer);
	memset(buffer, 0xa5, required);
	KUNIT_ASSERT_EQ(test, drm_constraints_snapshot_encode(view, buffer, required, &required), 0);
	header = (void *)buffer;
	KUNIT_EXPECT_EQ(test, header->version, DRM_MODE_CONSTRAINTS_VERSION);
	KUNIT_EXPECT_EQ(test, header->length, required);
	KUNIT_EXPECT_EQ(test, header->generation, 1);
	KUNIT_EXPECT_EQ(test, header->selected_id, drm_constraints_entry_id(f->initial));
	KUNIT_EXPECT_EQ(test, header->suggested_id, 0);
	KUNIT_EXPECT_EQ(test, header->count_entries, 1);
	KUNIT_EXPECT_EQ(test, header->entries_offset, sizeof(*header));
	KUNIT_EXPECT_EQ(test, header->entry_size, sizeof(*entry));
	KUNIT_EXPECT_EQ(test, header->pad | header->reserved[0] | header->reserved[1], 0);
	entry = (void *)(buffer + sizeof(*header));
	KUNIT_EXPECT_EQ(test, entry->id, header->selected_id);
	KUNIT_EXPECT_EQ(test, entry->flags, DRM_MODE_CONSTRAINTS_SELECTABLE);
	KUNIT_EXPECT_EQ(test, entry->description_offset, sizeof(*header) + sizeof(*entry));
	KUNIT_EXPECT_EQ(test, entry->description_offset + entry->description_length, required);
	KUNIT_EXPECT_EQ(test, entry->pad | entry->reserved[0] | entry->reserved[1], 0);
	description = (void *)((u8 *)entry + sizeof(*entry));
	KUNIT_EXPECT_EQ(test, description->version, DRM_MODE_CONSTRAINTS_VERSION);
	KUNIT_EXPECT_EQ(test, description->length, entry->description_length);
	KUNIT_EXPECT_EQ(test, description->record_count, 5);
	KUNIT_EXPECT_EQ(test, description->records_offset,
			entry->description_offset + sizeof(*description));
	output = (void *)((u8 *)description + sizeof(*description));
	KUNIT_EXPECT_EQ(test, output->header.type, DRM_MODE_CONSTRAINTS_RECORD_OUTPUT_SIZE);
	KUNIT_EXPECT_EQ(test, output->header.length, sizeof(*output));
	KUNIT_EXPECT_EQ(test, output->header.flags, DRM_MODE_CONSTRAINTS_RECORD_REQUIRED);
	KUNIT_EXPECT_EQ(test, output->min_width, 640);
	KUNIT_EXPECT_EQ(test, output->min_height, 360);
	KUNIT_EXPECT_EQ(test, output->max_width, 1920);
	KUNIT_EXPECT_EQ(test, output->max_height, 1080);
	format = (void *)((u8 *)output + sizeof(*output));
	KUNIT_EXPECT_EQ(test, format->header.type, DRM_MODE_CONSTRAINTS_RECORD_PLANE_FORMAT);
	KUNIT_EXPECT_EQ(test, format->header.length, sizeof(*format));
	KUNIT_EXPECT_EQ(test, format->header.flags, DRM_MODE_CONSTRAINTS_RECORD_REQUIRED);
	KUNIT_EXPECT_EQ(test, format->plane_id, 7);
	KUNIT_EXPECT_EQ(test, format->format, DRM_FORMAT_XRGB8888);
	KUNIT_EXPECT_EQ(test, format->modifier, implicit ? 0 : I915_FORMAT_MOD_X_TILED);
	KUNIT_EXPECT_EQ(test, format->layout_flags,
			 implicit ? DRM_MODE_CONSTRAINTS_LAYOUT_IMPLICIT : 0);
	KUNIT_EXPECT_EQ(test, format->storage_flags,
			DRM_MODE_CONSTRAINTS_FORMAT_STORAGE_IMPORTED);
	KUNIT_EXPECT_EQ(test, format->plane_count, 1);
	KUNIT_EXPECT_EQ(test, format->width_alignment, 64);
	KUNIT_EXPECT_EQ(test, format->height_alignment, 4);
	KUNIT_EXPECT_EQ(test, format->pitch_alignment, 256);
	KUNIT_EXPECT_EQ(test, format->offset_alignment, 4096);
	KUNIT_EXPECT_EQ(test, format->min_pitch, 1024);
	KUNIT_EXPECT_EQ(test, format->max_pitch, 65536);
	KUNIT_EXPECT_EQ(test, format->reserved, 0);
	KUNIT_EXPECT_EQ(test, format->min_width, 64);
	KUNIT_EXPECT_EQ(test, format->min_height, 32);
	KUNIT_EXPECT_EQ(test, format->max_width, 3840);
	KUNIT_EXPECT_EQ(test, format->max_height, 2160);
	property = (void *)((u8 *)format + sizeof(*format));
	KUNIT_EXPECT_EQ(test, property->header.type, DRM_MODE_CONSTRAINTS_RECORD_PROPERTY);
	KUNIT_EXPECT_EQ(test, property->header.length, sizeof(*property));
	KUNIT_EXPECT_EQ(test, property->header.flags, DRM_MODE_CONSTRAINTS_RECORD_REQUIRED);
	KUNIT_EXPECT_EQ(test, property->object_id, 7);
	KUNIT_EXPECT_EQ(test, property->property_id, 11);
	KUNIT_EXPECT_EQ(test, property->type, DRM_MODE_PROP_SIGNED_RANGE);
	KUNIT_EXPECT_EQ(test, property->applicability_flags,
			DRM_MODE_CONSTRAINTS_PROPERTY_PLANE_YUV);
	KUNIT_EXPECT_EQ(test, property->minimum, (u64)-10);
	KUNIT_EXPECT_EQ(test, property->maximum, 20);
	KUNIT_EXPECT_EQ(test, property->mask, 0);
	geometry = (void *)((u8 *)property + sizeof(*property));
	KUNIT_EXPECT_EQ(test, geometry->header.type,
			DRM_MODE_CONSTRAINTS_RECORD_PLANE_GEOMETRY);
	KUNIT_EXPECT_EQ(test, geometry->header.length, sizeof(*geometry));
	KUNIT_EXPECT_EQ(test, geometry->header.flags, DRM_MODE_CONSTRAINTS_RECORD_REQUIRED);
	KUNIT_EXPECT_EQ(test, geometry->plane_id, 7);
	KUNIT_EXPECT_EQ(test, geometry->flags, DRM_MODE_CONSTRAINTS_GEOMETRY_CROP |
			DRM_MODE_CONSTRAINTS_GEOMETRY_FRACTIONAL_SOURCE);
	KUNIT_EXPECT_EQ(test, geometry->min_scale, 1 << 15);
	KUNIT_EXPECT_EQ(test, geometry->max_scale, 1 << 17);
	plane_limit = (void *)((u8 *)geometry + sizeof(*geometry));
	KUNIT_EXPECT_EQ(test, plane_limit->header.type, DRM_MODE_CONSTRAINTS_RECORD_PLANE_LIMIT);
	KUNIT_EXPECT_EQ(test, plane_limit->header.length, 40);
	KUNIT_EXPECT_EQ(test, plane_limit->header.flags, DRM_MODE_CONSTRAINTS_RECORD_REQUIRED);
	KUNIT_EXPECT_EQ(test, plane_limit->max_active, 2);
	KUNIT_EXPECT_EQ(test, plane_limit->count_planes, 3);
	KUNIT_EXPECT_EQ(test, plane_limit->plane_ids[0], 7);
	KUNIT_EXPECT_EQ(test, plane_limit->plane_ids[1], 8);
	KUNIT_EXPECT_EQ(test, plane_limit->plane_ids[2], 9);
	KUNIT_EXPECT_EQ(test, plane_limit->plane_ids[3], 0);
	KUNIT_EXPECT_EQ(test,
			property->header.pad | geometry->header.pad | plane_limit->header.pad |
			format->header.pad | output->header.pad, 0);
}

static void encoding_preserves_native_metadata_without_padding(struct kunit *test)
{
	check_encoded_layout(test, false);
}

static void size_discovery_and_short_buffers_write_no_partial_payload(struct kunit *test)
{
	enum { bytes = sizeof(struct drm_mode_constraints_list) +
		       sizeof(struct drm_mode_constraints) +
		       sizeof(struct drm_mode_constraints_description) +
		       sizeof(struct drm_mode_constraints_output_size) +
		       sizeof(struct drm_mode_constraints_plane_format) +
		       sizeof(struct drm_mode_constraints_property) +
		       sizeof(struct drm_mode_constraints_plane_geometry) +
		       ALIGN(sizeof(struct drm_mode_constraints_plane_limit) +
			     3 * sizeof(__u32), 8) };
	struct encoding_fixture *f = new_fixture(test);
	struct drm_constraints_snapshot *view = snapshot(test, f->list);
	u8 buffer[bytes + 1], before[bytes + 1];
	size_t required = 123;

	memset(buffer, 0xa5, sizeof(buffer));
	memcpy(before, buffer, sizeof(buffer));
	KUNIT_EXPECT_EQ(test, drm_constraints_snapshot_encode(NULL, buffer, bytes, &required),
			-EINVAL);
	KUNIT_EXPECT_EQ(test, drm_constraints_snapshot_encode(view, NULL, 1, &required), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_constraints_snapshot_encode(view, buffer, bytes, NULL), -EINVAL);
	KUNIT_EXPECT_EQ(test, required, 123);
	KUNIT_EXPECT_MEMEQ(test, buffer, before, sizeof(buffer));
	KUNIT_EXPECT_EQ(test, drm_constraints_snapshot_encode(view, buffer, bytes - 1, &required),
			-ENOSPC);
	KUNIT_EXPECT_EQ(test, required, bytes);
	KUNIT_EXPECT_MEMEQ(test, buffer, before, sizeof(buffer));
	KUNIT_ASSERT_EQ(test, drm_constraints_snapshot_encode(view, buffer, bytes, &required), 0);
	KUNIT_EXPECT_EQ(test, buffer[bytes], 0xa5);
	/* Byte copies also support a deliberately unaligned kernel output buffer. */
	KUNIT_ASSERT_EQ(test, drm_constraints_snapshot_encode(view, before + 1, bytes, &required),
			0);
	KUNIT_EXPECT_EQ(test, before[0], 0xa5);
	KUNIT_EXPECT_MEMEQ(test, before + 1, buffer, bytes);
}

static void implicit_layout_is_not_encoded_as_explicit_linear(struct kunit *test)
{
	check_encoded_layout(test, true);
}

static void encoded_snapshot_remains_coherent_after_list_closure(struct kunit *test)
{
	struct encoding_fixture *f = new_fixture(test);
	struct drm_constraints_entry *target;
	struct drm_constraints_snapshot *view;
	struct drm_mode_constraints_list *header;
	struct drm_mode_constraints *entries;
	size_t required;
	u8 *before, *after;

	target = drm_constraints_entry_create(f->domain, 19, f->description, &ops, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, target);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_entry, target), 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_list_add(f->list, target), 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_list_suggest(f->list, drm_constraints_entry_id(target)), 0);
	view = snapshot(test, f->list);
	KUNIT_ASSERT_EQ(test, drm_constraints_snapshot_encode(view, NULL, 0, &required), 0);
	before = kunit_kmalloc(test, required, GFP_KERNEL);
	after = kunit_kmalloc(test, required, GFP_KERNEL);
	KUNIT_ASSERT_NOT_NULL(test, before);
	KUNIT_ASSERT_NOT_NULL(test, after);
	KUNIT_ASSERT_EQ(test, drm_constraints_snapshot_encode(view, before, required, &required), 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_list_withdraw(f->list, drm_constraints_entry_id(target)), 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_list_forget(f->list, drm_constraints_entry_id(target)), 0);
	drm_constraints_list_close(f->list);
	kunit_release_action(test, put_list, f->list);
	kunit_release_action(test, put_entry, target);
	KUNIT_ASSERT_EQ(test, drm_constraints_snapshot_encode(view, after, required, &required), 0);
	KUNIT_EXPECT_MEMEQ(test, before, after, required);
	header = (void *)after;
	entries = (void *)(after + sizeof(*header));
	KUNIT_EXPECT_EQ(test, header->entries_offset, sizeof(*header));
	KUNIT_EXPECT_EQ(test, header->count_entries, 2);
	KUNIT_EXPECT_EQ(test, header->generation, 3);
	KUNIT_EXPECT_EQ(test, header->suggested_id, entries[1].id);
	KUNIT_EXPECT_NE(test, entries[0].id, entries[1].id);
	KUNIT_EXPECT_EQ(test, entries[0].flags, DRM_MODE_CONSTRAINTS_SELECTABLE);
	KUNIT_EXPECT_EQ(test, entries[1].flags, DRM_MODE_CONSTRAINTS_SELECTABLE);
	KUNIT_EXPECT_EQ(test, entries[0].description_offset + entries[0].description_length,
			entries[1].description_offset);
	KUNIT_EXPECT_EQ(test, entries[1].description_offset + entries[1].description_length, required);
}

static void maximum_native_list_fits_bounded_encoding(struct kunit *test)
{
	const struct drm_constraints_size size = { 1, 1, 4096, 4096 };
	struct drm_constraints_domain *domain;
	struct drm_constraints_description *description;
	struct drm_constraints_list *list = NULL;
	struct drm_constraints_snapshot *view;
	struct drm_constraints_format *formats;
	struct drm_constraints_property *properties;
	struct drm_constraints_plane_geometry *geometries;
	struct drm_constraints_plane_limit *plane_limits;
	u32 *plane_ids;
	struct drm_mode_constraints_list *header;
	struct drm_mode_constraints *entries;
	size_t required, expected;
	unsigned int i;
	u8 *buffer;

	formats = kunit_kcalloc(test, DRM_CONSTRAINTS_MAX_FORMATS, sizeof(*formats), GFP_KERNEL);
	properties = kunit_kcalloc(test, DRM_CONSTRAINTS_MAX_PROPERTIES, sizeof(*properties), GFP_KERNEL);
	geometries = kunit_kcalloc(test, DRM_CONSTRAINTS_MAX_PLANE_GEOMETRIES,
				   sizeof(*geometries), GFP_KERNEL);
	plane_limits = kunit_kcalloc(test, DRM_CONSTRAINTS_MAX_PLANE_LIMITS,
				      sizeof(*plane_limits), GFP_KERNEL);
	plane_ids = kunit_kcalloc(test,
		DRM_CONSTRAINTS_MAX_PLANE_LIMITS * DRM_CONSTRAINTS_MAX_PLANES_PER_LIMIT,
		sizeof(*plane_ids), GFP_KERNEL);
	KUNIT_ASSERT_NOT_NULL(test, formats);
	KUNIT_ASSERT_NOT_NULL(test, properties);
	KUNIT_ASSERT_NOT_NULL(test, geometries);
	KUNIT_ASSERT_NOT_NULL(test, plane_limits);
	KUNIT_ASSERT_NOT_NULL(test, plane_ids);
	for (i = 0; i < DRM_CONSTRAINTS_MAX_FORMATS; i++)
		formats[i] = (struct drm_constraints_format) {
			.plane_id = i + 1, .format = DRM_FORMAT_XRGB8888,
			.modifier = DRM_FORMAT_MOD_LINEAR, .size = size,
			.storage_flags = DRM_CONSTRAINTS_FORMAT_STORAGE_NATIVE,
			.width_alignment = 1, .height_alignment = 1,
			.pitch_alignment = 1, .offset_alignment = 1,
			.min_pitch = 1, .max_pitch = U32_MAX,
		};
	for (i = 0; i < DRM_CONSTRAINTS_MAX_PROPERTIES; i++)
		properties[i] = (struct drm_constraints_property) {
			.object_id = 1, .property_id = i + 1,
			.type = DRM_MODE_PROP_RANGE, .maximum = 4096,
		};
	for (i = 0; i < DRM_CONSTRAINTS_MAX_PLANE_GEOMETRIES; i++)
		geometries[i] = (struct drm_constraints_plane_geometry) {
			.plane_id = i + 1,
			.min_scale = 1 << 16, .max_scale = 1 << 16,
		};
	for (i = 0; i < DRM_CONSTRAINTS_MAX_PLANE_LIMITS; i++) {
		unsigned int plane;

		plane_limits[i] = (struct drm_constraints_plane_limit) {
			.max_active = DRM_CONSTRAINTS_MAX_PLANES_PER_LIMIT,
			.count = DRM_CONSTRAINTS_MAX_PLANES_PER_LIMIT,
			.plane_ids = plane_ids + i * DRM_CONSTRAINTS_MAX_PLANES_PER_LIMIT,
		};
		for (plane = 0; plane < DRM_CONSTRAINTS_MAX_PLANES_PER_LIMIT; plane++)
			plane_ids[i * DRM_CONSTRAINTS_MAX_PLANES_PER_LIMIT + plane] = plane + 1;
	}
	description = drm_constraints_description_create_with_geometry(
		&size, formats, DRM_CONSTRAINTS_MAX_FORMATS,
		properties, DRM_CONSTRAINTS_MAX_PROPERTIES,
		plane_limits, DRM_CONSTRAINTS_MAX_PLANE_LIMITS,
		geometries, DRM_CONSTRAINTS_MAX_PLANE_GEOMETRIES);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, description);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_description, description), 0);
	domain = drm_constraints_domain_create(DRM_CONSTRAINTS_MAX_ENTRIES);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, domain);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_domain, domain), 0);
	for (i = 0; i < DRM_CONSTRAINTS_MAX_ENTRIES; i++) {
		struct drm_constraints_entry *entry =
			drm_constraints_entry_create(domain, 19, description, &ops, NULL);

		KUNIT_ASSERT_NOT_ERR_OR_NULL(test, entry);
		KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_entry, entry), 0);
		if (!list) {
			list = drm_constraints_list_create(domain, entry, DRM_CONSTRAINTS_MAX_ENTRIES);
			KUNIT_ASSERT_NOT_ERR_OR_NULL(test, list);
			KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_list, list), 0);
		} else {
			KUNIT_ASSERT_EQ(test, drm_constraints_list_add(list, entry), 0);
		}
	}
	view = snapshot(test, list);
	KUNIT_ASSERT_EQ(test, drm_constraints_snapshot_encode(view, NULL, 0, &required), 0);
	expected = sizeof(*header) + DRM_CONSTRAINTS_MAX_ENTRIES *
		(sizeof(*entries) + sizeof(struct drm_mode_constraints_description) +
		 sizeof(struct drm_mode_constraints_output_size) +
		 DRM_CONSTRAINTS_MAX_FORMATS * sizeof(struct drm_mode_constraints_plane_format) +
		 DRM_CONSTRAINTS_MAX_PROPERTIES * sizeof(struct drm_mode_constraints_property) +
		 DRM_CONSTRAINTS_MAX_PLANE_GEOMETRIES *
		 sizeof(struct drm_mode_constraints_plane_geometry) +
		 DRM_CONSTRAINTS_MAX_PLANE_LIMITS *
		 ALIGN(sizeof(struct drm_mode_constraints_plane_limit) +
		       DRM_CONSTRAINTS_MAX_PLANES_PER_LIMIT * sizeof(__u32), 8));
	KUNIT_EXPECT_EQ(test, required, expected);
	KUNIT_ASSERT_LE(test, required, DRM_MODE_CONSTRAINTS_MAX_BYTES);
	buffer = kvzalloc(required + 1, GFP_KERNEL);
	KUNIT_ASSERT_NOT_NULL(test, buffer);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, free_buffer, buffer), 0);
	buffer[required] = 0xa5;
	KUNIT_ASSERT_EQ(test, drm_constraints_snapshot_encode(view, buffer, required, &required), 0);
	KUNIT_EXPECT_EQ(test, buffer[required], 0xa5);
	header = (void *)buffer;
	entries = (void *)(buffer + sizeof(*header));
	KUNIT_EXPECT_EQ(test, header->entries_offset, sizeof(*header));
	KUNIT_EXPECT_EQ(test, header->count_entries, DRM_CONSTRAINTS_MAX_ENTRIES);
	KUNIT_EXPECT_EQ(test, entries[DRM_CONSTRAINTS_MAX_ENTRIES - 1].description_offset +
			entries[DRM_CONSTRAINTS_MAX_ENTRIES - 1].description_length, required);
}

static struct kunit_case drm_constraints_encoding_tests[] = {
	KUNIT_CASE(encoding_preserves_native_metadata_without_padding),
	KUNIT_CASE(size_discovery_and_short_buffers_write_no_partial_payload),
	KUNIT_CASE(implicit_layout_is_not_encoded_as_explicit_linear),
	KUNIT_CASE(encoded_snapshot_remains_coherent_after_list_closure),
	KUNIT_CASE(maximum_native_list_fits_bounded_encoding),
	{}
};

static struct kunit_suite drm_constraints_encoding_test_suite = {
	.name = "drm_constraints_encoding",
	.test_cases = drm_constraints_encoding_tests,
};

kunit_test_suite(drm_constraints_encoding_test_suite);

MODULE_DESCRIPTION("DRM constraints snapshot encoding tests");
MODULE_LICENSE("Dual MIT/GPL");
