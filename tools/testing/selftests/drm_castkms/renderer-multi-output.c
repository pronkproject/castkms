// SPDX-License-Identifier: GPL-2.0-only

/* Independent renderer and capture delivery across one atomic output cohort. */
#include "fixture.h"

#include <drm_fourcc.h>
#include <fcntl.h>
#include <linux/dma-buf.h>
#include <poll.h>
#include <stdbool.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <unistd.h>

#include "../../../../include/uapi/drm/castkms_drm.h"
#include "../../../../include/uapi/drm/drm_capture.h"
#include "../../../../include/uapi/drm/drm_constraints.h"

#define OUTPUTS 2

struct output {
	uint32_t crtc, connector;
	drmModeModeInfo mode;
	struct monitor_control monitor;
	struct buffer source, private, destination;
	int private_fd, destination_fd;
	struct drm_castkms_renderer_files renderer;
	uint64_t host, worker;
	struct drm_capture_grant_files capture;
};

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

static void select_cohort(int fd, struct output outputs[OUTPUTS], bool workers)
{
	drmModeAtomicReq *request = drmModeAtomicAlloc();

	CHECK(request);
	for (unsigned int i = 0; i < OUTPUTS; i++) {
		uint64_t id = workers ? outputs[i].worker : outputs[i].host;

		property(fd, request, outputs[i].crtc, DRM_MODE_OBJECT_CRTC,
			 DRM_CONSTRAINTS_ID_PROPERTY, id);
	}
	CHECK(drmModeAtomicCommit(fd, request, DRM_MODE_ATOMIC_ALLOW_MODESET, NULL) == 0);
	drmModeAtomicFree(request);
}

static void sync_dma_buf(int fd, uint64_t flags)
{
	struct dma_buf_sync request = { .flags = flags };

	CHECK(ioctl(fd, DMA_BUF_IOCTL_SYNC, &request) == 0);
}

static void copy_image(int source, size_t source_size, uint32_t source_pitch,
		       uint64_t source_offset, int destination, size_t destination_size,
		       uint32_t destination_pitch, uint64_t destination_offset,
		       uint32_t width, uint32_t height)
{
	unsigned char *from, *to;
	size_t row = (size_t)width * 4;

	CHECK(source_pitch >= row && destination_pitch >= row);
	CHECK(source_offset + (uint64_t)(height - 1) * source_pitch + row <= source_size);
	CHECK(destination_offset + (uint64_t)(height - 1) * destination_pitch + row <=
	      destination_size);
	from = mmap(NULL, source_size, PROT_READ, MAP_SHARED, source, 0);
	to = mmap(NULL, destination_size, PROT_WRITE, MAP_SHARED, destination, 0);
	CHECK(from != MAP_FAILED && to != MAP_FAILED);
	sync_dma_buf(source, DMA_BUF_SYNC_START | DMA_BUF_SYNC_READ);
	sync_dma_buf(destination, DMA_BUF_SYNC_START | DMA_BUF_SYNC_WRITE);
	for (uint32_t y = 0; y < height; y++)
		memcpy(to + destination_offset + (size_t)y * destination_pitch,
		       from + source_offset + (size_t)y * source_pitch, row);
	sync_dma_buf(destination, DMA_BUF_SYNC_END | DMA_BUF_SYNC_WRITE);
	sync_dma_buf(source, DMA_BUF_SYNC_END | DMA_BUF_SYNC_READ);
	CHECK(munmap(to, destination_size) == 0);
	CHECK(munmap(from, source_size) == 0);
}

static void check_pixels(const struct output *output, unsigned char value)
{
	unsigned char *pixels = mmap(NULL, output->destination.dumb.size, PROT_READ,
				     MAP_SHARED, output->destination_fd, 0);

	CHECK(pixels != MAP_FAILED);
	sync_dma_buf(output->destination_fd, DMA_BUF_SYNC_START | DMA_BUF_SYNC_READ);
	for (uint32_t y = 0; y < output->mode.vdisplay; y++)
		for (uint32_t x = 0; x < output->mode.hdisplay * 4; x++)
			if (x % 4 != 3)
				CHECK(pixels[(size_t)y *
					     output->destination.dumb.pitch + x] == value);
	sync_dma_buf(output->destination_fd, DMA_BUF_SYNC_END | DMA_BUF_SYNC_READ);
	CHECK(munmap(pixels, output->destination.dumb.size) == 0);
}

static void publish(int fd, struct output *output)
{
	struct drm_castkms_create_renderer_control create = {
		.crtc_id = output->crtc, .connector_id = output->connector,
		.files = (uintptr_t)&output->renderer,
	};
	struct {
		struct drm_castkms_renderer_constraints header;
		struct drm_castkms_renderer_constraints_format format;
	} constraints = {
		.header = {
			.version = DRM_CASTKMS_RENDERER_CONSTRAINTS_VERSION,
			.kind = DRM_CASTKMS_RENDERER_CONSTRAINTS_KIND, .format_count = 1,
			.min_scale = 1U << 16, .max_scale = 1U << 16,
			.max_layers = 1, .max_roles = { 1, 0, 0 },
		},
		.format = {
			.fourcc = DRM_FORMAT_XRGB8888, .plane_count = 1,
			.flags = DRM_CASTKMS_RENDERER_CONSTRAINTS_FORMAT_NATIVE,
			.roles = DRM_CASTKMS_RENDERER_CONSTRAINTS_ROLE_PRIMARY,
			.width_alignment = 1, .height_alignment = 1,
			.pitch_alignment = 1, .offset_alignment = 1,
			.min_pitch = 1, .max_pitch = 65536,
		},
	};
	struct drm_castkms_renderer_prepare_offer prepare = {
		.constraints = (uintptr_t)&constraints, .constraints_size = sizeof(constraints),
		.width = output->mode.hdisplay, .height = output->mode.vdisplay,
	};
	struct drm_castkms_renderer_register_image image = {
		.image_id = 1, .width = output->mode.hdisplay, .height = output->mode.vdisplay,
		.num_buffers = 1, .buffers = (uintptr_t)&output->private_fd,
	};
	struct drm_castkms_renderer_submit_probe probe = { .completion_fd = -1 };
	struct drm_castkms_renderer_offer_result result;
	struct drm_castkms_renderer_publish_offer offer = { .result = (uintptr_t)&result };
	uint32_t width = output->mode.hdisplay, height = output->mode.vdisplay;

	constraints.header.min_output[0] = width;
	constraints.header.max_output[0] = width;
	constraints.header.min_output[1] = height;
	constraints.header.max_output[1] = height;
	constraints.header.min_source[0] = width;
	constraints.header.max_source[0] = width;
	constraints.header.min_source[1] = height;
	constraints.header.max_source[1] = height;
	CHECK(ioctl(fd, DRM_IOCTL_CASTKMS_CREATE_RENDERER_CONTROL, &create) == 0);
	CHECK(fcntl(output->renderer.renderer_fd, F_GETFD) == FD_CLOEXEC);
	CHECK(fcntl(output->renderer.revoke_fd, F_GETFD) == FD_CLOEXEC);
	CHECK(ioctl(output->renderer.renderer_fd,
		    DRM_IOCTL_CASTKMS_RENDERER_PREPARE_OFFER, &prepare) == 0);
	CHECK(ioctl(output->renderer.renderer_fd,
		    DRM_IOCTL_CASTKMS_RENDERER_REGISTER_IMAGE, &image) == 0);
	CHECK(ioctl(output->renderer.renderer_fd,
		    DRM_IOCTL_CASTKMS_RENDERER_SUBMIT_PROBE, &probe) == 0);
	memset(&result, 0xa5, sizeof(result));
	CHECK(ioctl(output->renderer.renderer_fd,
		    DRM_IOCTL_CASTKMS_RENDERER_PUBLISH_OFFER, &offer) == 0);
	output->worker = result.constraints_id;
	CHECK(output->worker && output->worker != output->host);
	CHECK(!result.reserved[0] && !result.reserved[1] && !result.reserved[2]);
}

static void open_capture(int fd, struct output *output)
{
	struct drm_mode_create_capture_grant grant = {
		.crtc_id = output->crtc, .connector_id = output->connector,
		.files = (uintptr_t)&output->capture,
	};
	struct drm_capture_describe description;
	struct drm_capture_create_stream stream = { .id = 1, .capacity = 1 };
	struct drm_capture_register_destination destination = {
		.id = 1, .width = output->mode.hdisplay, .height = output->mode.vdisplay,
		.format = DRM_FORMAT_XRGB8888, .num_planes = 1,
		.modifier = DRM_FORMAT_MOD_LINEAR,
		.fds = { output->destination_fd },
		.strides = { output->destination.dumb.pitch },
	};
	struct drm_capture_queue_output queue = {
		.stream = 1, .use_id = 1, .destination = 1, .reuse_fd = -1,
	};

	CHECK(ioctl(fd, DRM_IOCTL_MODE_CREATE_CAPTURE_GRANT, &grant) == 0);
	CHECK(fcntl(output->capture.capture_fd, F_GETFD) == FD_CLOEXEC);
	CHECK(fcntl(output->capture.control_fd, F_GETFD) == FD_CLOEXEC);
	memset(&description, 0xa5, sizeof(description));
	CHECK(ioctl(output->capture.capture_fd, DRM_IOCTL_CAPTURE_DESCRIBE, &description) == 0);
	CHECK(description.width == output->mode.hdisplay &&
	      description.height == output->mode.vdisplay && !description.reserved);
	stream.offer = description.id;
	CHECK(ioctl(output->capture.capture_fd, DRM_IOCTL_CAPTURE_CREATE_STREAM, &stream) == 0);
	CHECK(ioctl(output->capture.capture_fd, DRM_IOCTL_CAPTURE_REGISTER_DESTINATION,
		    &destination) == 0);
	CHECK(ioctl(output->capture.capture_fd, DRM_IOCTL_CAPTURE_QUEUE_OUTPUT, &queue) == 0);
}

static void deliver(struct output *output, unsigned char value)
{
	struct drm_castkms_renderer_scene *scene = calloc(1, DRM_CASTKMS_RENDERER_SCENE_MAX_BYTES);
	struct drm_castkms_renderer_dequeue_scene dequeue = {
		.result = (uintptr_t)scene, .image_id = 1,
		.capacity = DRM_CASTKMS_RENDERER_SCENE_MAX_BYTES,
	};
	struct drm_castkms_renderer_layer *layer;
	struct drm_castkms_renderer_release_source source = {
		.completion_fd = -1, .kind = DRM_CASTKMS_RENDERER_RELEASE_CPU_DONE,
	};
	struct drm_castkms_renderer_output recipient;
	struct drm_castkms_renderer_dequeue_output take = {
		.result = (uintptr_t)&recipient, .image_id = 1,
	};
	struct drm_castkms_renderer_release_output release = {
		.completion_fd = -1, .kind = DRM_CASTKMS_RENDERER_RELEASE_CPU_DONE,
	};
	struct drm_capture_result result;
	struct drm_capture_dequeue capture = { .stream = 1, .result = (uintptr_t)&result };
	struct pollfd event = { .fd = output->capture.capture_fd, .events = POLLIN };

	CHECK(scene);
	CHECK(ioctl(output->renderer.renderer_fd,
		    DRM_IOCTL_CASTKMS_RENDERER_DEQUEUE_SCENE, &dequeue) == 0);
	CHECK(scene->version == DRM_CASTKMS_RENDERER_SCENE_VERSION);
	CHECK(scene->bytes == sizeof(*scene) + sizeof(*layer));
	CHECK(scene->constraints_id == output->worker && scene->layer_count == 1);
	CHECK(scene->producer_fd == -1);
	CHECK(scene->width == output->mode.hdisplay &&
	      scene->height == output->mode.vdisplay);
	CHECK(!scene->output_color_count && !scene->reserved);
	layer = (void *)(scene + 1);
	CHECK(layer->bytes == sizeof(*layer));
	CHECK(layer->kind == DRM_CASTKMS_RENDERER_LAYER_PRIMARY);
	CHECK(layer->format == DRM_FORMAT_XRGB8888 && layer->plane_count == 1);
	CHECK(layer->modifier == DRM_FORMAT_MOD_INVALID);
	CHECK(!layer->color_count && !layer->planes[0].reserved);
	CHECK(fcntl(layer->planes[0].dma_buf_fd, F_GETFD) == FD_CLOEXEC);
	copy_image(layer->planes[0].dma_buf_fd, output->source.dumb.size,
		   layer->planes[0].pitch, layer->planes[0].offset,
		   output->private_fd, output->private.dumb.size,
		   output->private.dumb.pitch, 0, output->mode.hdisplay,
		   output->mode.vdisplay);
	CHECK(close(layer->planes[0].dma_buf_fd) == 0);
	source.job_id = scene->job_id;
	CHECK(ioctl(output->renderer.renderer_fd,
		    DRM_IOCTL_CASTKMS_RENDERER_RELEASE_SOURCE, &source) == 0);
	memset(&recipient, 0xa5, sizeof(recipient));
	CHECK(ioctl(output->renderer.renderer_fd,
		    DRM_IOCTL_CASTKMS_RENDERER_DEQUEUE_OUTPUT, &take) == 0);
	CHECK(recipient.job_id && recipient.image_id == 1 &&
	      recipient.width == output->mode.hdisplay &&
	      recipient.height == output->mode.vdisplay);
	CHECK(recipient.format == DRM_FORMAT_XRGB8888 && recipient.plane_count == 1 &&
	      recipient.modifier == DRM_FORMAT_MOD_LINEAR);
	CHECK(!recipient.reserved[0] && !recipient.reserved[1]);
	CHECK(fcntl(recipient.dma_buf_fd, F_GETFD) == FD_CLOEXEC);
	copy_image(output->private_fd, output->private.dumb.size,
		   output->private.dumb.pitch, 0, recipient.dma_buf_fd,
		   output->destination.dumb.size, recipient.pitch, recipient.offset,
		   output->mode.hdisplay, output->mode.vdisplay);
	release.job_id = recipient.job_id;
	CHECK(ioctl(output->renderer.renderer_fd,
		    DRM_IOCTL_CASTKMS_RENDERER_RELEASE_OUTPUT, &release) == 0);
	CHECK(close(recipient.dma_buf_fd) == 0);
	CHECK(poll(&event, 1, 5000) == 1 && event.revents & POLLIN);
	memset(&result, 0xa5, sizeof(result));
	CHECK(ioctl(output->capture.capture_fd, DRM_IOCTL_CAPTURE_DEQUEUE, &capture) == 0);
	CHECK(result.use_id == 1 && result.status == 0 && result.completed_at_ns > 0 &&
	      !result.reserved);
	check_pixels(output, value);
	free(scene);
}

static void cleanup(int fd, struct output *output)
{
	struct drm_capture_unregister_destination destination = { .id = 1 };
	struct drm_capture_destroy_stream stream = { .id = 1 };
	struct drm_castkms_renderer_withdraw_offer withdraw = { 0 };
	struct drm_castkms_renderer_unregister_image image = { .image_id = 1 };

	CHECK(ioctl(output->capture.capture_fd, DRM_IOCTL_CAPTURE_UNREGISTER_DESTINATION,
		    &destination) == 0);
	CHECK(ioctl(output->capture.capture_fd, DRM_IOCTL_CAPTURE_DESTROY_STREAM, &stream) == 0);
	CHECK(close(output->capture.control_fd) == 0);
	CHECK(close(output->capture.capture_fd) == 0);
	CHECK(ioctl(output->renderer.renderer_fd,
		    DRM_IOCTL_CASTKMS_RENDERER_WITHDRAW_OFFER, &withdraw) == 0);
	CHECK(close(output->renderer.revoke_fd) == 0);
	CHECK(ioctl(output->renderer.renderer_fd,
		    DRM_IOCTL_CASTKMS_RENDERER_UNREGISTER_IMAGE, &image) == 0);
	CHECK(close(output->renderer.renderer_fd) == 0);
	CHECK(close(output->private_fd) == 0);
	CHECK(close(output->destination_fd) == 0);
	CHECK(drmModeSetCrtc(fd, output->crtc, 0, 0, 0, NULL, 0, NULL) == 0);
	destroy_buffer(fd, &output->source);
	destroy_buffer(fd, &output->private);
	destroy_buffer(fd, &output->destination);
	close_monitor(&output->monitor);
}

int main(int argc, char **argv)
{
	struct output outputs[OUTPUTS] = { 0 };
	drmModeRes *resources;
	int fd;

	if (argc != 2) {
		fprintf(stderr, "SKIP: supply a disposable two-output CastKMS node\n");
		return 4;
	}
	fd = open(argv[1], O_RDWR | O_CLOEXEC);
	CHECK(fd >= 0 && drmIsMaster(fd));
	CHECK(drmSetClientCap(fd, DRM_CLIENT_CAP_ATOMIC, 1) == 0);
	CHECK(drmSetClientCap(fd, DRM_CLIENT_CAP_KMS_CONSTRAINTS, 1) == 0);
	resources = drmModeGetResources(fd);
	CHECK(resources && resources->count_crtcs >= OUTPUTS &&
	      resources->count_connectors >= OUTPUTS);
	for (unsigned int i = 0; i < OUTPUTS; i++) {
		drmModeConnector *connector;

		outputs[i].crtc = resources->crtcs[i];
		outputs[i].connector = resources->connectors[i];
		outputs[i].renderer = (struct drm_castkms_renderer_files){ -1, -1 };
		outputs[i].capture = (struct drm_capture_grant_files){ -1, -1 };
		outputs[i].monitor = attach_fallback_monitor(fd, outputs[i].connector);
		outputs[i].host = selected(fd, outputs[i].crtc, 1);
		connector = drmModeGetConnector(fd, outputs[i].connector);
		CHECK(connector && connector->count_modes);
		outputs[i].mode = connector->modes[0];
		for (int m = 0; m < connector->count_modes; m++)
			if (connector->modes[m].hdisplay == 640 &&
			    connector->modes[m].vdisplay == 480)
				outputs[i].mode = connector->modes[m];
		drmModeFreeConnector(connector);
		outputs[i].source = create_buffer(fd, outputs[i].mode.hdisplay,
						  outputs[i].mode.vdisplay, 0x31 + i * 0x46);
		outputs[i].private = create_buffer(fd, outputs[i].mode.hdisplay,
						   outputs[i].mode.vdisplay, 0);
		outputs[i].destination = create_buffer(fd, outputs[i].mode.hdisplay,
						       outputs[i].mode.vdisplay, 0);
		CHECK(drmPrimeHandleToFD(fd, outputs[i].private.dumb.handle,
					 DRM_CLOEXEC | DRM_RDWR,
					 &outputs[i].private_fd) == 0);
		CHECK(drmPrimeHandleToFD(fd, outputs[i].destination.dumb.handle,
					 DRM_CLOEXEC | DRM_RDWR,
					 &outputs[i].destination_fd) == 0);
		publish(fd, &outputs[i]);
		CHECK(selected(fd, outputs[i].crtc, 2) == outputs[i].host);
		CHECK(drmModeSetCrtc(fd, outputs[i].crtc, outputs[i].source.fb, 0, 0,
				     &outputs[i].connector, 1, &outputs[i].mode) == 0);
	}
	select_cohort(fd, outputs, true);
	for (unsigned int i = 0; i < OUTPUTS; i++) {
		CHECK(selected(fd, outputs[i].crtc, 2) == outputs[i].worker);
		open_capture(fd, &outputs[i]);
	}
	/* Reverse delivery order to catch accidental device-global endpoint queues. */
	deliver(&outputs[1], 0x77);
	deliver(&outputs[0], 0x31);
	select_cohort(fd, outputs, false);
	for (unsigned int i = 0; i < OUTPUTS; i++)
		cleanup(fd, &outputs[i]);
	drmModeFreeResources(resources);
	CHECK(close(fd) == 0);
	puts("PASS: atomic two-output renderer selection and isolated capture delivery");
	return 0;
}
