// SPDX-License-Identifier: GPL-2.0-only

/* Exact non-linear renderer selection without interpreting synthetic pixels. */
#include "fixture.h"

#include <drm_fourcc.h>
#include <fcntl.h>
#include <poll.h>
#include <string.h>
#include <sys/ioctl.h>
#include <unistd.h>

#include "../../../../include/uapi/drm/castkms_drm.h"
#include "../../../../include/uapi/drm/drm_constraints.h"

static void check_static_envelope(int fd, uint32_t plane)
{
	drmModeObjectProperties *properties;
	drmModePropertyBlobRes *blob = NULL;
	struct drm_format_modifier_blob *header;
	struct drm_format_modifier *modifiers;
	uint32_t *formats;
	uint32_t xrgb = UINT32_MAX, rgbx = UINT32_MAX;
	bool xrgb_linear = false, rgbx_linear = false;

	properties = drmModeObjectGetProperties(fd, plane, DRM_MODE_OBJECT_PLANE);
	CHECK(properties);
	for (uint32_t i = 0; i < properties->count_props; i++) {
		drmModePropertyRes *property = drmModeGetProperty(fd, properties->props[i]);

		CHECK(property);
		if (!strcmp(property->name, "IN_FORMATS")) {
			CHECK(property->flags & DRM_MODE_PROP_BLOB);
			CHECK(properties->prop_values[i]);
			blob = drmModeGetPropertyBlob(fd, properties->prop_values[i]);
		}
		drmModeFreeProperty(property);
	}
	drmModeFreeObjectProperties(properties);
	CHECK(blob && blob->length >= sizeof(*header));
	header = blob->data;
	CHECK(header->version == FORMAT_BLOB_CURRENT);
	CHECK(header->formats_offset <= blob->length);
	CHECK(header->count_formats <=
	      (blob->length - header->formats_offset) / sizeof(*formats));
	CHECK(header->modifiers_offset <= blob->length);
	CHECK(header->count_modifiers <=
	      (blob->length - header->modifiers_offset) / sizeof(*modifiers));
	formats = (void *)((char *)blob->data + header->formats_offset);
	modifiers = (void *)((char *)blob->data + header->modifiers_offset);
	for (uint32_t i = 0; i < header->count_formats; i++) {
		if (formats[i] == DRM_FORMAT_XRGB8888)
			xrgb = i;
		if (formats[i] == DRM_FORMAT_RGBX8888)
			rgbx = i;
	}
	CHECK(xrgb != UINT32_MAX && rgbx != UINT32_MAX);
	for (uint32_t i = 0; i < header->count_modifiers; i++) {
		struct drm_format_modifier *modifier = &modifiers[i];

		CHECK(modifier->modifier != I915_FORMAT_MOD_4_TILED);
		CHECK(modifier->modifier == DRM_FORMAT_MOD_LINEAR);
		if (xrgb >= modifier->offset && xrgb - modifier->offset < 64 &&
		    modifier->formats & (1ULL << (xrgb - modifier->offset)))
			xrgb_linear = true;
		if (rgbx >= modifier->offset && rgbx - modifier->offset < 64 &&
		    modifier->formats & (1ULL << (rgbx - modifier->offset)))
			rgbx_linear = true;
	}
	CHECK(xrgb_linear && rgbx_linear);
	drmModeFreePropertyBlob(blob);
}

static uint64_t selected(int fd, uint32_t crtc, uint32_t count,
			 uint64_t *generation)
{
	struct drm_mode_list_constraints query = { .crtc_id = crtc };
	struct drm_mode_constraints_list *list;
	uint64_t id;

	CHECK(ioctl(fd, DRM_IOCTL_MODE_LIST_CONSTRAINTS, &query) == 0);
	list = calloc(1, query.size);
	CHECK(list);
	query.data = (uintptr_t)list;
	CHECK(ioctl(fd, DRM_IOCTL_MODE_LIST_CONSTRAINTS, &query) == 0);
	CHECK(list->version == DRM_MODE_CONSTRAINTS_VERSION);
	CHECK(list->count_entries == count && list->selected_id);
	CHECK(list->generation);
	if (generation)
		*generation = list->generation;
	id = list->selected_id;
	free(list);
	return id;
}

static uint64_t constraints_event(int fd, uint32_t crtc, uint64_t after)
{
	struct drm_event_kms_constraints_list_changed event;
	struct pollfd waiter = { .fd = fd, .events = POLLIN };

	CHECK(poll(&waiter, 1, 2000) == 1);
	CHECK(waiter.revents & POLLIN);
	CHECK(!(waiter.revents & (POLLERR | POLLHUP | POLLNVAL)));
	CHECK(read(fd, &event, sizeof(event)) == sizeof(event));
	CHECK(event.base.type == DRM_EVENT_KMS_CONSTRAINTS_LIST_CHANGED);
	CHECK(event.base.length == sizeof(event));
	CHECK(event.crtc_id == crtc);
	CHECK(!event.flags && !event.reserved);
	CHECK(event.generation > after);
	return event.generation;
}

static void expect_no_constraints_event(int fd)
{
	struct pollfd waiter = { .fd = fd, .events = POLLIN };

	CHECK(poll(&waiter, 1, 0) == 0);
}

static uint32_t property_id(int fd, uint32_t object, const char *name)
{
	drmModeObjectProperties *properties;
	uint32_t id = 0;

	properties = drmModeObjectGetProperties(fd, object, DRM_MODE_OBJECT_PLANE);
	CHECK(properties);
	for (uint32_t i = 0; i < properties->count_props; i++) {
		drmModePropertyRes *property = drmModeGetProperty(fd, properties->props[i]);

		CHECK(property);
		if (!strcmp(property->name, name))
			id = property->prop_id;
		drmModeFreeProperty(property);
	}
	drmModeFreeObjectProperties(properties);
	CHECK(id);
	return id;
}

static void check_offer_rules(int fd, uint32_t crtc, uint64_t id, uint32_t plane)
{
	struct drm_mode_list_constraints query = { .crtc_id = crtc };
	struct drm_mode_constraints_list *list;
	struct drm_mode_constraints *entries;
	static const char * const names[] = {
		"COLOR_ENCODING", "COLOR_RANGE",
	};
	uint32_t ids[2];
	bool found[2] = { 0 }, geometry_found = false;
	unsigned int count = 0, total = 0, geometries = 0, plane_limits = 0;

	for (unsigned int i = 0; i < 2; i++)
		ids[i] = property_id(fd, plane, names[i]);
	CHECK(ioctl(fd, DRM_IOCTL_MODE_LIST_CONSTRAINTS, &query) == 0);
	list = calloc(1, query.size);
	CHECK(list);
	query.data = (uintptr_t)list;
	CHECK(ioctl(fd, DRM_IOCTL_MODE_LIST_CONSTRAINTS, &query) == 0);
	CHECK(list->version == DRM_MODE_CONSTRAINTS_VERSION);
	CHECK(list->length == query.size);
	CHECK(list->entry_size == sizeof(*entries));
	CHECK(list->entries_offset <= list->length);
	CHECK(list->count_entries <=
	      (list->length - list->entries_offset) / sizeof(*entries));
	entries = (void *)((char *)list + list->entries_offset);
	for (uint32_t i = 0; i < list->count_entries; i++) {
		struct drm_mode_constraints_description *description;
		char *cursor, *end;

		if (entries[i].id != id)
			continue;
		CHECK(entries[i].description_offset <= list->length);
		CHECK(entries[i].description_length <=
		      list->length - entries[i].description_offset);
		description = (void *)((char *)list + entries[i].description_offset);
		CHECK(entries[i].description_length >= sizeof(*description));
		CHECK(description->length == entries[i].description_length);
		CHECK(description->records_offset >= entries[i].description_offset +
		      sizeof(*description));
		CHECK(description->records_offset <= list->length);
		cursor = (char *)list + description->records_offset;
		end = (char *)description + description->length;
		CHECK(cursor <= end);
		for (uint32_t record = 0; record < description->record_count; record++) {
			struct drm_mode_constraints_record *header = (void *)cursor;

			CHECK((size_t)(end - cursor) >= sizeof(*header));
			CHECK(header->length >= sizeof(*header));
			CHECK(header->length <= (size_t)(end - cursor));
			if (header->type == DRM_MODE_CONSTRAINTS_RECORD_PROPERTY) {
				struct drm_mode_constraints_property *property = (void *)header;

				CHECK(header->length == sizeof(*property));
				CHECK(header->flags == DRM_MODE_CONSTRAINTS_RECORD_REQUIRED);
				total++;
				if (property->object_id != plane) {
					cursor += header->length;
					continue;
				}
				count++;
				for (unsigned int rule = 0; rule < 2; rule++) {
					if (property->property_id != ids[rule])
						continue;
					CHECK(!found[rule]);
					found[rule] = true;
					CHECK(property->type == DRM_MODE_PROP_ENUM);
					CHECK(property->applicability_flags ==
					      DRM_MODE_CONSTRAINTS_PROPERTY_PLANE_YUV);
					CHECK(!property->minimum && !property->maximum);
					CHECK(property->mask == (rule ? 0x3 : 0x7));
				}
			} else if (header->type == DRM_MODE_CONSTRAINTS_RECORD_PLANE_GEOMETRY) {
				struct drm_mode_constraints_plane_geometry *geometry;

				geometry = (void *)header;
				CHECK(header->length == sizeof(*geometry));
				CHECK(header->flags == DRM_MODE_CONSTRAINTS_RECORD_REQUIRED);
				CHECK(!geometry->flags);
				CHECK(geometry->min_scale == (1U << 16));
				CHECK(geometry->max_scale == (1U << 16));
				geometries++;
				if (geometry->plane_id == plane) {
					CHECK(!geometry_found);
					geometry_found = true;
				}
			} else if (header->type == DRM_MODE_CONSTRAINTS_RECORD_PLANE_LIMIT) {
				struct drm_mode_constraints_plane_limit *limit = (void *)header;
				size_t payload, length;
				bool has_primary = false;

				CHECK(limit->count_planes <=
				      DRM_MODE_CONSTRAINTS_MAX_PLANES_PER_LIMIT);
				payload = sizeof(*limit) + limit->count_planes * sizeof(uint32_t);
				length = (payload + 7) & ~7;
				CHECK(header->flags == DRM_MODE_CONSTRAINTS_RECORD_REQUIRED);
				CHECK(header->length == length);
				CHECK(limit->max_active == 1);
				for (uint32_t member = 0; member < limit->count_planes; member++) {
					CHECK(limit->plane_ids[member]);
					has_primary |= limit->plane_ids[member] == plane;
					for (uint32_t previous = 0; previous < member; previous++)
						CHECK(limit->plane_ids[member] !=
						      limit->plane_ids[previous]);
				}
				for (size_t padding = payload; padding < length; padding++)
					CHECK(!cursor[padding]);
				if (has_primary)
					CHECK(limit->count_planes == 9);
				else
					CHECK(limit->count_planes == 8);
				plane_limits++;
			}
			cursor += header->length;
		}
		CHECK(cursor == end);
	}
	CHECK(count == 2 && total == 18);
	CHECK(geometry_found && geometries == 9);
	CHECK(plane_limits == 2);
	for (unsigned int i = 0; i < 2; i++)
		CHECK(found[i]);
	free(list);
}

static void check_offer_format(int fd, uint32_t crtc, uint64_t id,
			       uint32_t plane, uint32_t fourcc, uint64_t modifier,
			       uint32_t plane_count, uint32_t storage_flags,
			       uint32_t width, uint32_t height)
{
	struct drm_mode_list_constraints query = { .crtc_id = crtc };
	struct drm_mode_constraints_list *list;
	struct drm_mode_constraints *entries;
	bool found = false;
	unsigned int outputs = 0;

	CHECK(ioctl(fd, DRM_IOCTL_MODE_LIST_CONSTRAINTS, &query) == 0);
	list = calloc(1, query.size);
	CHECK(list);
	query.data = (uintptr_t)list;
	CHECK(ioctl(fd, DRM_IOCTL_MODE_LIST_CONSTRAINTS, &query) == 0);
	CHECK(list->version == DRM_MODE_CONSTRAINTS_VERSION);
	CHECK(list->length == query.size);
	CHECK(list->entry_size == sizeof(*entries));
	CHECK(list->entries_offset <= list->length);
	CHECK(list->count_entries <=
	      (list->length - list->entries_offset) / sizeof(*entries));
	entries = (void *)((char *)list + list->entries_offset);
	for (uint32_t i = 0; i < list->count_entries; i++) {
		struct drm_mode_constraints_description *description;
		char *cursor, *end;

		if (entries[i].id != id)
			continue;
		CHECK(entries[i].flags == DRM_MODE_CONSTRAINTS_SELECTABLE);
		CHECK(entries[i].description_offset <= list->length);
		CHECK(entries[i].description_length <=
		      list->length - entries[i].description_offset);
		description = (void *)((char *)list + entries[i].description_offset);
		CHECK(entries[i].description_length >= sizeof(*description));
		CHECK(description->version == DRM_MODE_CONSTRAINTS_VERSION);
		CHECK(description->length == entries[i].description_length);
		CHECK(description->records_offset >= entries[i].description_offset +
		      sizeof(*description));
		CHECK(description->records_offset <= list->length);
		cursor = (char *)list + description->records_offset;
		end = (char *)description + description->length;
		CHECK(cursor <= end);
		for (uint32_t record = 0; record < description->record_count; record++) {
			struct drm_mode_constraints_record *header = (void *)cursor;

			CHECK((size_t)(end - cursor) >= sizeof(*header));
			CHECK(header->length >= sizeof(*header));
			CHECK(header->length <= (size_t)(end - cursor));
			if (header->type == DRM_MODE_CONSTRAINTS_RECORD_OUTPUT_SIZE) {
				struct drm_mode_constraints_output_size *output = (void *)header;

				CHECK(header->length == sizeof(*output));
				CHECK(header->flags == DRM_MODE_CONSTRAINTS_RECORD_REQUIRED);
				CHECK(output->min_width == width && output->max_width == width);
				CHECK(output->min_height == height && output->max_height == height);
				outputs++;
			} else if (header->type == DRM_MODE_CONSTRAINTS_RECORD_PLANE_FORMAT) {
				struct drm_mode_constraints_plane_format *format = (void *)header;

				CHECK(header->length == sizeof(*format));
				if (format->plane_id == plane && format->format == fourcc &&
				    format->modifier == modifier) {
					CHECK(!format->layout_flags);
					CHECK(format->storage_flags == storage_flags);
					CHECK(format->plane_count == plane_count);
					CHECK(format->pitch_alignment == 1);
					CHECK(format->offset_alignment == 1);
					CHECK(format->min_pitch == 256);
					CHECK(format->max_pitch == 65536);
					CHECK(!format->reserved);
					CHECK(format->width_alignment == 64);
					CHECK(format->height_alignment == 4);
					CHECK(format->min_width == width &&
					      format->max_width == width);
					CHECK(format->min_height == height &&
					      format->max_height == height);
					found = true;
				}
			}
			cursor += header->length;
		}
		CHECK(cursor == end);
	}
	CHECK(found && outputs == 1);
	free(list);
}

struct multiplane_buffer {
	struct drm_mode_create_dumb dumb[2];
	uint32_t fb;
};

static void select_framebuffer(int fd, uint32_t crtc, uint32_t plane,
			       uint32_t framebuffer, uint64_t constraints)
{
	drmModeAtomicReq *request = drmModeAtomicAlloc();

	CHECK(request);
	property(fd, request, plane, DRM_MODE_OBJECT_PLANE, "FB_ID", framebuffer);
	property(fd, request, crtc, DRM_MODE_OBJECT_CRTC,
		 DRM_CONSTRAINTS_ID_PROPERTY, constraints);
	CHECK(drmModeAtomicCommit(fd, request, DRM_MODE_ATOMIC_ALLOW_MODESET, NULL) == 0);
	drmModeAtomicFree(request);
}

static void reject_host_framebuffer(int fd, uint32_t plane, uint32_t framebuffer)
{
	drmModeAtomicReq *request = drmModeAtomicAlloc();

	CHECK(request);
	property(fd, request, plane, DRM_MODE_OBJECT_PLANE, "FB_ID", framebuffer);
	errno = 0;
	CHECK(drmModeAtomicCommit(fd, request, 0, NULL) < 0);
	CHECK(errno == EINVAL);
	drmModeAtomicFree(request);
}

static struct buffer create_tiled_buffer(int fd, uint32_t width, uint32_t height,
					 uint32_t format)
{
	struct buffer buffer = create_buffer(fd, width, height, 0);
	uint32_t handles[4] = { buffer.dumb.handle };
	uint32_t pitches[4] = { buffer.dumb.pitch };
	uint32_t offsets[4] = { 0 };
	uint64_t modifiers[4] = { I915_FORMAT_MOD_4_TILED };

	CHECK(drmModeRmFB(fd, buffer.fb) == 0);
	buffer.fb = 0;
	CHECK(drmModeAddFB2WithModifiers(fd, width, height, format,
				       handles, pitches, offsets, modifiers,
				       &buffer.fb, DRM_MODE_FB_MODIFIERS) == 0);
	return buffer;
}

static struct multiplane_buffer create_tiled_nv12(int fd, uint32_t width,
						   uint32_t height)
{
	struct multiplane_buffer buffer = {
		.dumb = {
			{ .width = width, .height = height, .bpp = 8 },
			{ .width = width, .height = height / 2, .bpp = 8 },
		},
	};
	uint32_t handles[4] = { 0 };
	uint32_t pitches[4] = { 0 };
	uint32_t offsets[4] = { 0 };
	uint64_t modifiers[4] = {
		I915_FORMAT_MOD_4_TILED,
		I915_FORMAT_MOD_4_TILED,
	};

	CHECK(!(width & 1) && !(height & 1));
	for (unsigned int i = 0; i < 2; i++) {
		CHECK(drmIoctl(fd, DRM_IOCTL_MODE_CREATE_DUMB, &buffer.dumb[i]) == 0);
		handles[i] = buffer.dumb[i].handle;
		pitches[i] = buffer.dumb[i].pitch;
	}
	CHECK(drmModeAddFB2WithModifiers(fd, width, height, DRM_FORMAT_NV12,
				       handles, pitches, offsets, modifiers,
				       &buffer.fb, DRM_MODE_FB_MODIFIERS) == 0);
	return buffer;
}

static void destroy_multiplane_buffer(int fd, struct multiplane_buffer *buffer)
{
	CHECK(drmModeRmFB(fd, buffer->fb) == 0);
	for (unsigned int i = 0; i < 2; i++) {
		struct drm_mode_destroy_dumb destroy = { .handle = buffer->dumb[i].handle };

		CHECK(drmIoctl(fd, DRM_IOCTL_MODE_DESTROY_DUMB, &destroy) == 0);
	}
}

int main(int argc, char **argv)
{
	struct drm_castkms_renderer_files files = { .renderer_fd = -1, .revoke_fd = -1 };
	struct drm_castkms_create_renderer_control create = { .files = (uintptr_t)&files };
	struct {
		struct drm_castkms_renderer_constraints header;
		struct drm_castkms_renderer_constraints_format formats[3];
	} constraints = {
		.header = {
			.version = DRM_CASTKMS_RENDERER_CONSTRAINTS_VERSION,
			.kind = DRM_CASTKMS_RENDERER_CONSTRAINTS_KIND,
			.format_count = 3,
			.min_scale = 1U << 16,
			.max_scale = 1U << 16,
			.max_layers = 1,
			.max_roles = { 1, 1, 0 },
			.yuv_encodings = DRM_CASTKMS_RENDERER_CONSTRAINTS_YUV_ENCODING_BT601 |
				 DRM_CASTKMS_RENDERER_CONSTRAINTS_YUV_ENCODING_BT709 |
				 DRM_CASTKMS_RENDERER_CONSTRAINTS_YUV_ENCODING_BT2020,
			.yuv_ranges = DRM_CASTKMS_RENDERER_CONSTRAINTS_YUV_RANGE_LIMITED |
				 DRM_CASTKMS_RENDERER_CONSTRAINTS_YUV_RANGE_FULL,
		},
		.formats = {
			{
				.fourcc = DRM_FORMAT_XRGB8888,
				.plane_count = 1,
				.modifier = I915_FORMAT_MOD_4_TILED,
				.flags = DRM_CASTKMS_RENDERER_CONSTRAINTS_FORMAT_NATIVE |
					 DRM_CASTKMS_RENDERER_CONSTRAINTS_FORMAT_IMPORTED |
					 DRM_CASTKMS_RENDERER_CONSTRAINTS_FORMAT_EXPLICIT_MODIFIER,
				.width_alignment = 64,
				.height_alignment = 4,
				.pitch_alignment = 1,
				.offset_alignment = 1,
				.min_pitch = 256,
				.max_pitch = 65536,
			},
			{
				.fourcc = DRM_FORMAT_NV12,
				.plane_count = 2,
				.modifier = I915_FORMAT_MOD_4_TILED,
				.flags = DRM_CASTKMS_RENDERER_CONSTRAINTS_FORMAT_NATIVE |
					 DRM_CASTKMS_RENDERER_CONSTRAINTS_FORMAT_EXPLICIT_MODIFIER,
				.width_alignment = 64,
				.height_alignment = 4,
				.pitch_alignment = 1,
				.offset_alignment = 1,
				.min_pitch = 256,
				.max_pitch = 65536,
			},
			{
				.fourcc = DRM_FORMAT_RGBX8888,
				.plane_count = 1,
				.modifier = I915_FORMAT_MOD_4_TILED,
				.flags = DRM_CASTKMS_RENDERER_CONSTRAINTS_FORMAT_NATIVE |
					 DRM_CASTKMS_RENDERER_CONSTRAINTS_FORMAT_EXPLICIT_MODIFIER,
				.width_alignment = 64,
				.height_alignment = 4,
				.pitch_alignment = 1,
				.offset_alignment = 1,
				.min_pitch = 256,
				.max_pitch = 65536,
			},
		},
	};
	struct drm_castkms_renderer_prepare_offer prepare = {
		.constraints = (uintptr_t)&constraints,
		.constraints_size = sizeof(constraints),
	};
	struct drm_castkms_renderer_submit_probe probe = { .completion_fd = -1 };
	struct drm_castkms_renderer_offer_result result;
	struct drm_castkms_renderer_publish_offer publish = { .result = (uintptr_t)&result };
	struct drm_castkms_renderer_register_image image = { .image_id = 1, .num_buffers = 1 };
	struct drm_castkms_renderer_unregister_image remove = { .image_id = 1 };
	struct drm_castkms_renderer_dequeue_scene dequeue = {
		.image_id = 1,
		.capacity = DRM_CASTKMS_RENDERER_SCENE_MAX_BYTES,
	};
	struct drm_castkms_renderer_release_source release = {
		.completion_fd = -1,
		.kind = DRM_CASTKMS_RENDERER_RELEASE_NO_ACCESS,
	};
	struct drm_castkms_renderer_withdraw_offer withdraw = { 0 };
	struct drm_castkms_renderer_scene *scene;
	struct drm_castkms_renderer_layer *layer;
	drmModeRes *resources;
	drmModeConnector *connector;
	drmModeModeInfo *mode;
	struct monitor_control monitor;
	struct buffer linear, tiled, rgbx, imported = { 0 }, private;
	struct multiplane_buffer nv12;
	uint64_t host, worker, generation, event_generation;
	uint64_t content_serial, job_id;
	uint32_t plane;
	int fd, private_fd;

	if (argc != 2 && argc != 3) {
		fprintf(stderr, "SKIP: supply a disposable CastKMS node [DMA heap]\n");
		return 4;
	}
	fd = open(argv[1], O_RDWR | O_CLOEXEC);
	CHECK(fd >= 0 && drmIsMaster(fd));
	CHECK(drmSetClientCap(fd, DRM_CLIENT_CAP_ATOMIC, 1) == 0);
	CHECK(drmSetClientCap(fd, DRM_CLIENT_CAP_KMS_CONSTRAINTS, 1) == 0);
	resources = drmModeGetResources(fd);
	CHECK(resources && resources->count_crtcs && resources->count_connectors);
	create.crtc_id = resources->crtcs[0];
	create.connector_id = resources->connectors[0];
	monitor = attach_fallback_monitor(fd, create.connector_id);
	host = selected(fd, create.crtc_id, 1, &generation);
	connector = drmModeGetConnector(fd, create.connector_id);
	CHECK(connector && connector->count_modes);
	mode = &connector->modes[0];
	for (int i = 0; i < connector->count_modes; i++)
		if (connector->modes[i].hdisplay == 640 &&
		    connector->modes[i].vdisplay == 480)
			mode = &connector->modes[i];
	plane = primary_plane(fd, 0);
	check_static_envelope(fd, plane);
	linear = create_buffer(fd, mode->hdisplay, mode->vdisplay, 0);
	tiled = create_tiled_buffer(fd, mode->hdisplay, mode->vdisplay,
				    DRM_FORMAT_XRGB8888);
	rgbx = create_tiled_buffer(fd, mode->hdisplay, mode->vdisplay,
				   DRM_FORMAT_RGBX8888);
	nv12 = create_tiled_nv12(fd, mode->hdisplay, mode->vdisplay);
	if (argc == 3)
		imported = import_xrgb_buffer_with_modifier(fd, argv[2], mode->hdisplay,
							    mode->vdisplay,
							    I915_FORMAT_MOD_4_TILED);
	private = create_buffer(fd, mode->hdisplay, mode->vdisplay, 0);
	CHECK(drmPrimeHandleToFD(fd, private.dumb.handle, DRM_CLOEXEC | DRM_RDWR,
			       &private_fd) == 0);
	CHECK(drmModeSetCrtc(fd, create.crtc_id, linear.fb, 0, 0,
			     &create.connector_id, 1, mode) == 0);
	reject_host_framebuffer(fd, plane, tiled.fb);
	CHECK(selected(fd, create.crtc_id, 1, NULL) == host);

	/* The published offer must narrow broad implementation limits to this pool. */
	constraints.header.min_output[0] = 1;
	constraints.header.max_output[0] = 16384;
	constraints.header.min_output[1] = 1;
	constraints.header.max_output[1] = 16384;
	constraints.header.min_source[0] = mode->hdisplay;
	constraints.header.max_source[0] = mode->hdisplay;
	constraints.header.min_source[1] = mode->vdisplay;
	constraints.header.max_source[1] = mode->vdisplay;
	prepare.width = mode->hdisplay;
	prepare.height = mode->vdisplay;
	image.width = mode->hdisplay;
	image.height = mode->vdisplay;
	image.buffers = (uintptr_t)&private_fd;
	CHECK(ioctl(fd, DRM_IOCTL_CASTKMS_CREATE_RENDERER_CONTROL, &create) == 0);
	CHECK(fcntl(files.renderer_fd, F_GETFD) == FD_CLOEXEC);
	CHECK(fcntl(files.revoke_fd, F_GETFD) == FD_CLOEXEC);
	CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_PREPARE_OFFER,
		    &prepare) == 0);
	CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_REGISTER_IMAGE,
		    &image) == 0);
	CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_SUBMIT_PROBE,
		    &probe) == 0);
	memset(&result, 0xa5, sizeof(result));
	CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_PUBLISH_OFFER,
		    &publish) == 0);
	worker = result.constraints_id;
	CHECK(worker && worker != host && !result.reserved[0] &&
	      !result.reserved[1] && !result.reserved[2]);
	event_generation = constraints_event(fd, create.crtc_id, generation);
	CHECK(selected(fd, create.crtc_id, 2, &generation) == host);
	CHECK(generation == event_generation);
	check_offer_format(fd, create.crtc_id, worker, plane, DRM_FORMAT_XRGB8888,
			   I915_FORMAT_MOD_4_TILED, 1,
			   DRM_MODE_CONSTRAINTS_FORMAT_STORAGE_NATIVE |
			   DRM_MODE_CONSTRAINTS_FORMAT_STORAGE_IMPORTED,
			   mode->hdisplay, mode->vdisplay);
	check_offer_format(fd, create.crtc_id, worker, plane, DRM_FORMAT_NV12,
			   I915_FORMAT_MOD_4_TILED, 2,
			   DRM_MODE_CONSTRAINTS_FORMAT_STORAGE_NATIVE,
			   mode->hdisplay, mode->vdisplay);
	check_offer_format(fd, create.crtc_id, worker, plane, DRM_FORMAT_RGBX8888,
			   I915_FORMAT_MOD_4_TILED, 1,
			   DRM_MODE_CONSTRAINTS_FORMAT_STORAGE_NATIVE,
			   mode->hdisplay, mode->vdisplay);
	check_offer_rules(fd, create.crtc_id, worker, plane);
	select_framebuffer(fd, create.crtc_id, plane, tiled.fb, worker);
	event_generation = constraints_event(fd, create.crtc_id, generation);
	CHECK(selected(fd, create.crtc_id, 2, &generation) == worker);
	CHECK(generation == event_generation);

	scene = calloc(1, dequeue.capacity);
	CHECK(scene);
	dequeue.result = (uintptr_t)scene;
	CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_DEQUEUE_SCENE,
		    &dequeue) == 0);
	CHECK(scene->version == DRM_CASTKMS_RENDERER_SCENE_VERSION);
	CHECK(scene->constraints_id == worker && scene->layer_count == 1);
	CHECK(scene->width == mode->hdisplay && scene->height == mode->vdisplay);
	CHECK(scene->producer_fd == -1 && !scene->output_color_count && !scene->reserved);
	layer = (void *)(scene + 1);
	CHECK(scene->bytes == sizeof(*scene) + sizeof(*layer));
	CHECK(layer->bytes == sizeof(*layer) && layer->plane_count == 1);
	CHECK(layer->kind == DRM_CASTKMS_RENDERER_LAYER_PRIMARY);
	CHECK(layer->format == DRM_FORMAT_XRGB8888);
	CHECK(layer->modifier == I915_FORMAT_MOD_4_TILED);
	CHECK(layer->width == mode->hdisplay && layer->height == mode->vdisplay);
	CHECK(!layer->source[0] && !layer->source[1]);
	CHECK(layer->source[2] == (uint32_t)mode->hdisplay << 16);
	CHECK(layer->source[3] == (uint32_t)mode->vdisplay << 16);
	CHECK(!layer->position[0] && !layer->position[1]);
	CHECK(layer->destination[0] == mode->hdisplay &&
	      layer->destination[1] == mode->vdisplay);
	CHECK(!layer->color_count && !layer->planes[0].offset &&
	      !layer->planes[0].reserved);
	CHECK(layer->planes[0].pitch == tiled.dumb.pitch);
	CHECK(fcntl(layer->planes[0].dma_buf_fd, F_GETFD) == FD_CLOEXEC);
	CHECK(close(layer->planes[0].dma_buf_fd) == 0);
	content_serial = scene->content_serial;
	job_id = scene->job_id;
	release.job_id = scene->job_id;
	CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_RELEASE_SOURCE,
		    &release) == 0);
	expect_no_constraints_event(fd);

	select_framebuffer(fd, create.crtc_id, plane, nv12.fb, worker);
	memset(scene, 0, dequeue.capacity);
	CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_DEQUEUE_SCENE,
		    &dequeue) == 0);
	CHECK(scene->version == DRM_CASTKMS_RENDERER_SCENE_VERSION);
	CHECK(scene->constraints_id == worker && scene->layer_count == 1);
	CHECK(scene->content_serial > content_serial && scene->job_id != job_id);
	CHECK(scene->width == mode->hdisplay && scene->height == mode->vdisplay);
	CHECK(scene->producer_fd == -1 && !scene->output_color_count && !scene->reserved);
	layer = (void *)(scene + 1);
	CHECK(scene->bytes == sizeof(*scene) + sizeof(*layer));
	CHECK(layer->bytes == sizeof(*layer) && layer->plane_count == 2);
	CHECK(layer->kind == DRM_CASTKMS_RENDERER_LAYER_PRIMARY);
	CHECK(layer->format == DRM_FORMAT_NV12);
	CHECK(layer->modifier == I915_FORMAT_MOD_4_TILED);
	CHECK(layer->width == mode->hdisplay && layer->height == mode->vdisplay);
	CHECK(!layer->source[0] && !layer->source[1]);
	CHECK(layer->source[2] == (uint32_t)mode->hdisplay << 16);
	CHECK(layer->source[3] == (uint32_t)mode->vdisplay << 16);
	CHECK(!layer->position[0] && !layer->position[1]);
	CHECK(layer->destination[0] == mode->hdisplay &&
	      layer->destination[1] == mode->vdisplay);
	CHECK(layer->color_encoding == DRM_CASTKMS_YUV_ENCODING_BT601);
	CHECK(layer->color_range == DRM_CASTKMS_YUV_RANGE_FULL);
	CHECK(!layer->color_count);
	CHECK(layer->planes[0].pitch == nv12.dumb[0].pitch);
	CHECK(layer->planes[1].pitch == nv12.dumb[1].pitch);
	CHECK(layer->planes[0].dma_buf_fd != layer->planes[1].dma_buf_fd);
	for (unsigned int i = 0; i < 2; i++) {
		CHECK(!layer->planes[i].offset && !layer->planes[i].reserved);
		CHECK(fcntl(layer->planes[i].dma_buf_fd, F_GETFD) == FD_CLOEXEC);
		CHECK(close(layer->planes[i].dma_buf_fd) == 0);
	}
	for (unsigned int i = 2; i < DRM_CASTKMS_RENDERER_MAX_PLANES; i++)
		CHECK(layer->planes[i].dma_buf_fd == -1 && !layer->planes[i].pitch &&
		      !layer->planes[i].offset && !layer->planes[i].reserved);
	content_serial = scene->content_serial;
	job_id = scene->job_id;
	release.job_id = scene->job_id;
	CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_RELEASE_SOURCE,
		    &release) == 0);

	select_framebuffer(fd, create.crtc_id, plane, rgbx.fb, worker);
	memset(scene, 0, dequeue.capacity);
	CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_DEQUEUE_SCENE,
		    &dequeue) == 0);
	layer = (void *)(scene + 1);
	CHECK(scene->version == DRM_CASTKMS_RENDERER_SCENE_VERSION);
	CHECK(scene->constraints_id == worker && scene->layer_count == 1);
	CHECK(scene->content_serial > content_serial && scene->job_id != job_id);
	CHECK(scene->width == mode->hdisplay && scene->height == mode->vdisplay);
	CHECK(scene->producer_fd == -1 && !scene->output_color_count && !scene->reserved);
	CHECK(scene->bytes == sizeof(*scene) + sizeof(*layer));
	CHECK(layer->bytes == sizeof(*layer) && layer->plane_count == 1);
	CHECK(layer->kind == DRM_CASTKMS_RENDERER_LAYER_PRIMARY);
	CHECK(layer->format == DRM_FORMAT_RGBX8888);
	CHECK(layer->modifier == I915_FORMAT_MOD_4_TILED);
	CHECK(layer->width == mode->hdisplay && layer->height == mode->vdisplay);
	CHECK(!layer->source[0] && !layer->source[1]);
	CHECK(layer->source[2] == (uint32_t)mode->hdisplay << 16);
	CHECK(layer->source[3] == (uint32_t)mode->vdisplay << 16);
	CHECK(!layer->position[0] && !layer->position[1]);
	CHECK(layer->destination[0] == mode->hdisplay &&
	      layer->destination[1] == mode->vdisplay);
	CHECK(!layer->color_count && !layer->planes[0].offset &&
	      !layer->planes[0].reserved);
	CHECK(layer->planes[0].pitch == rgbx.dumb.pitch);
	CHECK(fcntl(layer->planes[0].dma_buf_fd, F_GETFD) == FD_CLOEXEC);
	CHECK(close(layer->planes[0].dma_buf_fd) == 0);
	content_serial = scene->content_serial;
	job_id = scene->job_id;
	release.job_id = scene->job_id;
	CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_RELEASE_SOURCE,
		    &release) == 0);
	if (argc == 3) {
		select_framebuffer(fd, create.crtc_id, plane, imported.fb, worker);
		memset(scene, 0, dequeue.capacity);
		CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_DEQUEUE_SCENE,
			    &dequeue) == 0);
		layer = (void *)(scene + 1);
		CHECK(scene->version == DRM_CASTKMS_RENDERER_SCENE_VERSION);
		CHECK(scene->constraints_id == worker && scene->layer_count == 1);
		CHECK(scene->content_serial > content_serial && scene->job_id != job_id);
		CHECK(scene->width == mode->hdisplay && scene->height == mode->vdisplay);
		CHECK(scene->producer_fd == -1 && !scene->output_color_count &&
		      !scene->reserved);
		CHECK(scene->bytes == sizeof(*scene) + sizeof(*layer));
		CHECK(layer->bytes == sizeof(*layer) && layer->plane_count == 1);
		CHECK(layer->kind == DRM_CASTKMS_RENDERER_LAYER_PRIMARY);
		CHECK(layer->format == DRM_FORMAT_XRGB8888);
		CHECK(layer->modifier == I915_FORMAT_MOD_4_TILED);
		CHECK(layer->width == mode->hdisplay && layer->height == mode->vdisplay);
		CHECK(!layer->source[0] && !layer->source[1]);
		CHECK(layer->source[2] == (uint32_t)mode->hdisplay << 16);
		CHECK(layer->source[3] == (uint32_t)mode->vdisplay << 16);
		CHECK(!layer->position[0] && !layer->position[1]);
		CHECK(layer->destination[0] == mode->hdisplay &&
		      layer->destination[1] == mode->vdisplay);
		CHECK(!layer->color_count && !layer->planes[0].offset &&
		      !layer->planes[0].reserved);
		CHECK(layer->planes[0].pitch == mode->hdisplay * 4);
		CHECK(fcntl(layer->planes[0].dma_buf_fd, F_GETFD) == FD_CLOEXEC);
		CHECK(close(layer->planes[0].dma_buf_fd) == 0);
		release.job_id = scene->job_id;
		CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_RELEASE_SOURCE,
			    &release) == 0);
	}

	select_framebuffer(fd, create.crtc_id, plane, linear.fb, host);
	event_generation = constraints_event(fd, create.crtc_id, generation);
	CHECK(selected(fd, create.crtc_id, 2, &generation) == host);
	CHECK(generation == event_generation);
	CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_WITHDRAW_OFFER,
		    &withdraw) == 0);
	event_generation = constraints_event(fd, create.crtc_id, generation);
	CHECK(selected(fd, create.crtc_id, 2, &generation) == host);
	CHECK(generation == event_generation);
	CHECK(close(files.revoke_fd) == 0);
	CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_UNREGISTER_IMAGE,
		    &remove) == 0);
	CHECK(close(files.renderer_fd) == 0);
	CHECK(close(private_fd) == 0);
	CHECK(drmModeSetCrtc(fd, create.crtc_id, 0, 0, 0, NULL, 0, NULL) == 0);
	destroy_buffer(fd, &linear);
	destroy_buffer(fd, &tiled);
	destroy_buffer(fd, &rgbx);
	destroy_multiplane_buffer(fd, &nv12);
	if (argc == 3)
		destroy_buffer(fd, &imported);
	destroy_buffer(fd, &private);
	free(scene);
	drmModeFreeConnector(connector);
	drmModeFreeResources(resources);
	close_monitor(&monitor);
	CHECK(close(fd) == 0);
	puts("PASS: exact tiled renderer selection through public UAPI");
	return 0;
}
