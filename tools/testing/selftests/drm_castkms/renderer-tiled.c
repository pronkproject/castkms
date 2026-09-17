// SPDX-License-Identifier: GPL-2.0-only

/* Exact non-linear renderer selection without interpreting synthetic pixels. */
#include "fixture.h"

#include <drm_fourcc.h>
#include <fcntl.h>
#include <string.h>
#include <sys/ioctl.h>
#include <unistd.h>

#include "../../../../include/uapi/drm/castkms_drm.h"
#include "../../../../include/uapi/drm/drm_constraints.h"

static uint64_t selected(int fd, uint32_t crtc, uint32_t count)
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
	id = list->selected_id;
	free(list);
	return id;
}

static void check_offer_format(int fd, uint32_t crtc, uint64_t id,
			       uint32_t plane, uint32_t fourcc, uint64_t modifier,
			       uint32_t width, uint32_t height)
{
	struct drm_mode_list_constraints query = { .crtc_id = crtc };
	struct drm_mode_constraints_list *list;
	struct drm_mode_constraints *entries;
	bool found = false;

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
			if (header->type == DRM_MODE_CONSTRAINTS_RECORD_PLANE_FORMAT) {
				struct drm_mode_constraints_plane_format *format = (void *)header;

				CHECK(header->length == sizeof(*format));
				if (format->plane_id == plane && format->format == fourcc &&
				    format->modifier == modifier) {
					CHECK(!format->layout_flags);
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
	CHECK(found);
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
			.max_roles = { 1, 0, 0 },
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
				.pitch_alignment = 1,
				.offset_alignment = 1,
				.max_pitch = 65536,
			},
			{
				.fourcc = DRM_FORMAT_NV12,
				.plane_count = 2,
				.modifier = I915_FORMAT_MOD_4_TILED,
				.flags = DRM_CASTKMS_RENDERER_CONSTRAINTS_FORMAT_NATIVE |
					 DRM_CASTKMS_RENDERER_CONSTRAINTS_FORMAT_EXPLICIT_MODIFIER,
				.pitch_alignment = 1,
				.offset_alignment = 1,
				.max_pitch = 65536,
			},
			{
				.fourcc = DRM_FORMAT_RGBX8888,
				.plane_count = 1,
				.modifier = I915_FORMAT_MOD_4_TILED,
				.flags = DRM_CASTKMS_RENDERER_CONSTRAINTS_FORMAT_NATIVE |
					 DRM_CASTKMS_RENDERER_CONSTRAINTS_FORMAT_EXPLICIT_MODIFIER,
				.pitch_alignment = 1,
				.offset_alignment = 1,
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
	uint64_t host, worker;
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
	host = selected(fd, create.crtc_id, 1);
	connector = drmModeGetConnector(fd, create.connector_id);
	CHECK(connector && connector->count_modes);
	mode = &connector->modes[0];
	for (int i = 0; i < connector->count_modes; i++)
		if (connector->modes[i].hdisplay == 640 &&
		    connector->modes[i].vdisplay == 480)
			mode = &connector->modes[i];
	plane = primary_plane(fd, 0);
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
	CHECK(selected(fd, create.crtc_id, 1) == host);

	constraints.header.min_output[0] = mode->hdisplay;
	constraints.header.max_output[0] = mode->hdisplay;
	constraints.header.min_output[1] = mode->vdisplay;
	constraints.header.max_output[1] = mode->vdisplay;
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
	CHECK(selected(fd, create.crtc_id, 2) == host);
	check_offer_format(fd, create.crtc_id, worker, plane, DRM_FORMAT_XRGB8888,
			   I915_FORMAT_MOD_4_TILED, mode->hdisplay, mode->vdisplay);
	check_offer_format(fd, create.crtc_id, worker, plane, DRM_FORMAT_NV12,
			   I915_FORMAT_MOD_4_TILED, mode->hdisplay, mode->vdisplay);
	check_offer_format(fd, create.crtc_id, worker, plane, DRM_FORMAT_RGBX8888,
			   I915_FORMAT_MOD_4_TILED, mode->hdisplay, mode->vdisplay);
	select_framebuffer(fd, create.crtc_id, plane, tiled.fb, worker);
	CHECK(selected(fd, create.crtc_id, 2) == worker);

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
	CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_WITHDRAW_OFFER,
		    &withdraw) == 0);
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
