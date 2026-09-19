// SPDX-License-Identifier: GPL-2.0-only

/* Immutable renderer configuration, source execution and capture delivery. */
#include "fixture.h"

#include <dirent.h>
#include <drm_fourcc.h>
#include <fcntl.h>
#include <linux/dma-buf.h>
#include <poll.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <unistd.h>

#include "../../../../include/uapi/drm/castkms_drm.h"
#include "../../../../include/uapi/drm/drm_capture.h"
#include "../../../../include/uapi/drm/drm_constraints.h"

_Static_assert(sizeof(struct drm_castkms_renderer_files) == 8, "renderer files layout");
_Static_assert(sizeof(struct drm_castkms_create_renderer) == 32, "create layout");
_Static_assert(sizeof(struct drm_castkms_renderer_query) == 32, "query layout");
_Static_assert(sizeof(struct drm_castkms_renderer_configure) == 48, "configure layout");
_Static_assert(sizeof(struct drm_castkms_renderer_publish) == 32, "publish layout");
_Static_assert(sizeof(struct drm_castkms_renderer_publish_result) == 32, "result layout");
_Static_assert(sizeof(struct drm_castkms_renderer_withdraw) == 16, "withdraw layout");
_Static_assert(sizeof(struct drm_castkms_renderer_register_image) == 48,
	       "image registration layout");
_Static_assert(sizeof(struct drm_castkms_renderer_unregister_image) == 16,
	       "image removal layout");
_Static_assert(sizeof(struct drm_castkms_renderer_acquire_job) == 32,
	       "job acquire layout");
_Static_assert(sizeof(struct drm_castkms_renderer_constraints) == 128, "constraints layout");
_Static_assert(sizeof(struct drm_castkms_renderer_constraints_format) == 56, "format layout");
_Static_assert(sizeof(struct drm_castkms_renderer_job) == 56, "job layout");
_Static_assert(sizeof(struct drm_castkms_renderer_plane) == 144, "plane layout");
_Static_assert(sizeof(struct drm_castkms_renderer_memory_plane) == 16, "memory plane layout");
_Static_assert(sizeof(struct drm_castkms_renderer_color_op) == 8, "color op layout");
_Static_assert(sizeof(struct drm_castkms_renderer_release_job) == 32, "job release layout");
_Static_assert(sizeof(struct drm_castkms_renderer_acquire_output) == 32, "output acquire layout");
_Static_assert(sizeof(struct drm_castkms_renderer_output) == 72, "output layout");
_Static_assert(sizeof(struct drm_castkms_renderer_release_output) == 32, "output release layout");
_Static_assert(sizeof(struct drm_capture_queue_output) == 40, "capture queue layout");
_Static_assert(sizeof(struct drm_capture_result) == 24, "capture result layout");
_Static_assert(sizeof(struct drm_capture_cancel) == 24, "capture cancel layout");

struct sw_sync_create_fence_data {
	uint32_t value;
	char name[32];
	int32_t fence;
};

#define SW_SYNC_IOC_CREATE_FENCE \
	_IOWR('W', 0, struct sw_sync_create_fence_data)
#define SW_SYNC_IOC_INC _IOW('W', 1, uint32_t)

#define expect_error(fd, cmd, request, error) do { \
	errno = 0; \
	CHECK(ioctl((fd), (cmd), (request)) < 0); \
	CHECK(errno == (error)); \
} while (0)

static int pending_fence(int *timeline)
{
	struct sw_sync_create_fence_data create = {
		.value = 1,
		.name = "castkms-ready",
		.fence = -1,
	};

	*timeline = open("/sys/kernel/debug/sync/sw_sync", O_RDWR | O_CLOEXEC);
	CHECK(*timeline >= 0);
	CHECK(ioctl(*timeline, SW_SYNC_IOC_CREATE_FENCE, &create) == 0);
	CHECK(create.fence >= 0);
	CHECK(fcntl(create.fence, F_GETFD) == FD_CLOEXEC);
	return create.fence;
}

static void signal_fence(int timeline)
{
	uint32_t increment = 1;

	CHECK(ioctl(timeline, SW_SYNC_IOC_INC, &increment) == 0);
}

static unsigned int open_files(void)
{
	DIR *directory = opendir("/proc/self/fd");
	struct dirent *entry;
	unsigned int count = 0;

	CHECK(directory);
	while ((entry = readdir(directory))) {
		char *end;
		long fd = strtol(entry->d_name, &end, 10);

		if (!*end && fd >= 0 && fd != dirfd(directory))
			count++;
	}
	CHECK(closedir(directory) == 0);
	return count;
}

static uint64_t selected(int fd, uint32_t crtc, unsigned int count)
{
	struct drm_mode_list_constraints query = { .crtc_id = crtc };
	struct drm_mode_constraints_list *list;
	uint64_t id;

	CHECK(ioctl(fd, DRM_IOCTL_MODE_LIST_CONSTRAINTS, &query) == 0);
	CHECK(query.size >= sizeof(*list));
	list = calloc(1, query.size);
	CHECK(list);
	query.data = (uintptr_t)list;
	CHECK(ioctl(fd, DRM_IOCTL_MODE_LIST_CONSTRAINTS, &query) == 0);
	CHECK(list->version == DRM_MODE_CONSTRAINTS_VERSION && list->count_entries == count);
	id = list->selected_id;
	CHECK(id);
	free(list);
	return id;
}

static void select_backend(int fd, uint32_t crtc, uint64_t id, uint32_t flags)
{
	drmModeAtomicReq *req = drmModeAtomicAlloc();

	CHECK(req);
	property(fd, req, crtc, DRM_MODE_OBJECT_CRTC, DRM_CONSTRAINTS_ID_PROPERTY, id);
	CHECK(drmModeAtomicCommit(fd, req, flags, NULL) == 0);
	drmModeAtomicFree(req);
}

static struct drm_castkms_renderer_query query_endpoint(int fd)
{
	struct drm_castkms_renderer_query query;

	memset(&query, 0xa5, sizeof(query));
	CHECK(ioctl(fd, DRM_IOCTL_CASTKMS_RENDERER_QUERY, &query) == 0);
	CHECK(query.version == DRM_CASTKMS_RENDERER_VERSION);
	CHECK(!query.reserved[0] && !query.reserved[1]);
	return query;
}

static void readable(int fd, int expected)
{
	struct pollfd pollfd = { .fd = fd, .events = POLLIN };

	CHECK(poll(&pollfd, 1, 0) >= 0);
	CHECK(!(pollfd.revents & (POLLHUP | POLLERR | POLLNVAL)));
	CHECK(!!(pollfd.revents & POLLIN) == expected);
}

static void hung_up(int fd)
{
	struct pollfd pollfd = { .fd = fd, .events = POLLIN };

	CHECK(poll(&pollfd, 1, 0) == 1);
	CHECK((pollfd.revents & (POLLHUP | POLLERR)) == (POLLHUP | POLLERR));
	CHECK(!(pollfd.revents & POLLNVAL));
}

static void close_job(struct drm_castkms_renderer_job *job)
{
	char *cursor = (char *)(job + 1);

	if (job->acquire_fence_fd >= 0) {
		CHECK(fcntl(job->acquire_fence_fd, F_GETFD) == FD_CLOEXEC);
		CHECK(close(job->acquire_fence_fd) == 0);
	}
	for (uint32_t i = 0; i < job->plane_count; i++) {
		struct drm_castkms_renderer_plane *job_plane = (void *)cursor;

		CHECK(job_plane->bytes >= sizeof(*job_plane) && job_plane->memory_plane_count <= 4);
		for (uint32_t plane = 0; plane < job_plane->memory_plane_count; plane++) {
			CHECK(fcntl(job_plane->memory_planes[plane].dma_buf_fd,
				    F_GETFD) == FD_CLOEXEC);
			CHECK(close(job_plane->memory_planes[plane].dma_buf_fd) == 0);
		}
		cursor += job_plane->bytes;
		CHECK(cursor <= (char *)job + job->bytes);
	}
}

static struct drm_castkms_renderer_plane *
single_plane(struct drm_castkms_renderer_job *job, uint64_t constraints_id,
	     uint32_t width, uint32_t height, uint64_t previous)
{
	struct drm_castkms_renderer_plane *job_plane = (void *)(job + 1);

	CHECK(job->version == DRM_CASTKMS_RENDERER_JOB_VERSION);
	CHECK(job->constraints_id == constraints_id && job->content_serial > previous);
	CHECK(job->bytes == sizeof(*job) + sizeof(*job_plane));
	CHECK(job->width == width && job->height == height);
	CHECK(job->plane_count == 1 && job->acquire_fence_fd == -1);
	CHECK(!job->output_color_op_count && !job->reserved);
	CHECK(job_plane->bytes == sizeof(*job_plane));
	CHECK(job_plane->role == DRM_CASTKMS_RENDERER_PLANE_PRIMARY);
	CHECK(job_plane->format == DRM_FORMAT_XRGB8888);
	CHECK(job_plane->modifier == DRM_FORMAT_MOD_INVALID && job_plane->memory_plane_count == 1);
	CHECK(job_plane->width == width && job_plane->height == height);
	CHECK(job_plane->src_x == 0 && job_plane->src_y == 0);
	CHECK(job_plane->src_w == width << 16 && job_plane->src_h == height << 16);
	CHECK(job_plane->crtc_x == 0 && job_plane->crtc_y == 0);
	CHECK(job_plane->crtc_w == width && job_plane->crtc_h == height);
	CHECK(!job_plane->color_op_count && !job_plane->memory_planes[0].reserved);
	return job_plane;
}

static void check_layout(size_t size, uint64_t offset, uint32_t pitch,
			 uint32_t width, uint32_t height)
{
	size_t row = (size_t)width * 4;

	CHECK(width && height && row / 4 == width);
	CHECK(pitch >= row && offset <= size && row <= size - offset);
	CHECK((size_t)(height - 1) <= (size - offset - row) / pitch);
}

static void copy_linear(int source, size_t source_size, uint32_t source_pitch,
			uint64_t source_offset, int destination, size_t destination_size,
			uint32_t destination_pitch, uint64_t destination_offset,
			uint32_t width, uint32_t height)
{
	struct dma_buf_sync source_sync = { .flags = DMA_BUF_SYNC_START | DMA_BUF_SYNC_READ };
	struct dma_buf_sync destination_sync = { .flags = DMA_BUF_SYNC_START | DMA_BUF_SYNC_WRITE };
	unsigned char *source_pixels, *destination_pixels;
	size_t row = (size_t)width * 4;

	check_layout(source_size, source_offset, source_pitch, width, height);
	check_layout(destination_size, destination_offset, destination_pitch, width, height);
	source_pixels = mmap(NULL, source_size, PROT_READ, MAP_SHARED, source, 0);
	destination_pixels = mmap(NULL, destination_size, PROT_WRITE, MAP_SHARED,
				  destination, 0);
	CHECK(source_pixels != MAP_FAILED && destination_pixels != MAP_FAILED);
	CHECK(ioctl(source, DMA_BUF_IOCTL_SYNC, &source_sync) == 0);
	CHECK(ioctl(destination, DMA_BUF_IOCTL_SYNC, &destination_sync) == 0);
	for (uint32_t y = 0; y < height; y++)
		memcpy(destination_pixels + destination_offset + (size_t)y * destination_pitch,
		       source_pixels + source_offset + (size_t)y * source_pitch, row);
	destination_sync.flags = DMA_BUF_SYNC_END | DMA_BUF_SYNC_WRITE;
	source_sync.flags = DMA_BUF_SYNC_END | DMA_BUF_SYNC_READ;
	CHECK(ioctl(destination, DMA_BUF_IOCTL_SYNC, &destination_sync) == 0);
	CHECK(ioctl(source, DMA_BUF_IOCTL_SYNC, &source_sync) == 0);
	CHECK(munmap(destination_pixels, destination_size) == 0);
	CHECK(munmap(source_pixels, source_size) == 0);
}

static void check_pixels(int dma, const struct buffer *buffer, uint32_t width,
			 uint32_t height, unsigned char value)
{
	struct dma_buf_sync sync = { .flags = DMA_BUF_SYNC_START | DMA_BUF_SYNC_READ };
	unsigned char *pixels = mmap(NULL, buffer->dumb.size, PROT_READ, MAP_SHARED, dma, 0);

	CHECK(pixels != MAP_FAILED);
	CHECK(ioctl(dma, DMA_BUF_IOCTL_SYNC, &sync) == 0);
	for (uint32_t y = 0; y < height; y++)
		for (uint32_t x = 0; x < width * 4; x++)
			if (x % 4 != 3)
				CHECK(pixels[(size_t)y * buffer->dumb.pitch + x] == value);
	sync.flags = DMA_BUF_SYNC_END | DMA_BUF_SYNC_READ;
	CHECK(ioctl(dma, DMA_BUF_IOCTL_SYNC, &sync) == 0);
	CHECK(munmap(pixels, buffer->dumb.size) == 0);
}

static void wait_capture(int fd)
{
	struct pollfd event = { .fd = fd, .events = POLLIN };

	CHECK(poll(&event, 1, 5000) == 1);
	CHECK(event.revents & POLLIN);
	CHECK(!(event.revents & (POLLHUP | POLLERR | POLLNVAL)));
}

static void render_one(int renderer, struct drm_castkms_renderer_acquire_job *acquire,
		       struct drm_castkms_renderer_job *job,
		       struct drm_castkms_renderer_release_job *release,
		       uint64_t constraints_id, uint64_t *previous,
		       int private_fd, const struct buffer *private)
{
	struct drm_castkms_renderer_plane *job_plane;

	readable(renderer, 1);
	CHECK(ioctl(renderer, DRM_IOCTL_CASTKMS_RENDERER_ACQUIRE_JOB, acquire) == 0);
	job_plane = single_plane(job, constraints_id, private->dumb.width,
				 private->dumb.height, *previous);
	*previous = job->content_serial;
	release->job_id = job->job_id;
	expect_error(renderer, DRM_IOCTL_CASTKMS_RENDERER_ACQUIRE_JOB, acquire, EBUSY);
	copy_linear(job_plane->memory_planes[0].dma_buf_fd, private->dumb.size,
		    job_plane->memory_planes[0].pitch, job_plane->memory_planes[0].offset,
		    private_fd, private->dumb.size, private->dumb.pitch, 0,
		    private->dumb.width, private->dumb.height);
	close_job(job);
	release->kind = DRM_CASTKMS_RENDERER_RELEASE_CPU_DONE;
	CHECK(ioctl(renderer, DRM_IOCTL_CASTKMS_RENDERER_RELEASE_JOB, release) == 0);
}

int main(int argc, char **argv)
{
	struct drm_castkms_renderer_files files = { .renderer_fd = -1, .revoke_fd = -1 };
	struct drm_castkms_create_renderer create = { .files = (uintptr_t)&files };
	struct {
		struct drm_castkms_renderer_constraints header;
		struct drm_castkms_renderer_constraints_format format;
	} constraints = {
		.header = {
			.version = DRM_CASTKMS_RENDERER_CONSTRAINTS_VERSION,
			.kind = DRM_CASTKMS_RENDERER_CONSTRAINTS_KIND, .format_count = 1,
			.min_scale = 1U << 16, .max_scale = 1U << 16,
			.max_planes = 1, .max_roles = { 1, 0, 0 },
		},
		.format = {
			.fourcc = DRM_FORMAT_XRGB8888, .memory_plane_count = 1,
			.flags = DRM_CASTKMS_RENDERER_CONSTRAINTS_FORMAT_NATIVE,
			.roles = DRM_CASTKMS_RENDERER_CONSTRAINTS_ROLE_PRIMARY,
			.width_alignment = 1, .height_alignment = 1,
			.pitch_alignment = 1, .offset_alignment = 1,
			.min_pitch = 1, .max_pitch = 65536,
		},
	};
	struct drm_castkms_renderer_configure configure = {
		.constraints = (uintptr_t)&constraints, .constraints_size = sizeof(constraints),
	};
	struct drm_castkms_renderer_publish_result result, untouched;
	struct drm_castkms_renderer_publish publish = {
		.result = (uintptr_t)&result,
		.ready_fence_fd = -1,
	};
	struct drm_castkms_renderer_withdraw withdraw = { 0 };
	struct drm_castkms_renderer_register_image image = { .image_id = 1, .num_buffers = 1 };
	struct drm_castkms_renderer_unregister_image remove = { .image_id = 1 };
	struct drm_castkms_renderer_acquire_job acquire = {
		.target_image_id = 1, .capacity = DRM_CASTKMS_RENDERER_JOB_MAX_BYTES,
	};
	struct drm_castkms_renderer_release_job release = {
		.release_fence_fd = -1, .kind = DRM_CASTKMS_RENDERER_RELEASE_NO_ACCESS,
	};
	struct drm_castkms_renderer_acquire_output take_output = { .image_id = 1 };
	struct drm_castkms_renderer_output renderer_output;
	struct drm_castkms_renderer_release_output release_output = {
		.release_fence_fd = -1, .kind = DRM_CASTKMS_RENDERER_RELEASE_CPU_DONE,
	};
	struct drm_capture_grant_files capture_files = { .capture_fd = -1, .control_fd = -1 };
	struct drm_mode_create_capture_grant grant = { .files = (uintptr_t)&capture_files };
	struct drm_capture_describe description = {};
	struct drm_capture_create_stream stream = { .id = 1, .capacity = 1 };
	struct drm_capture_register_destination destination = {
		.id = 1, .format = DRM_FORMAT_XRGB8888, .num_planes = 1,
		.modifier = DRM_FORMAT_MOD_LINEAR,
	};
	struct drm_capture_queue_output queue = {
		.stream = 1, .use_id = 1, .destination = 1, .reuse_fd = -1,
	};
	struct drm_capture_dequeue take_capture = { .stream = 1 };
	struct drm_capture_cancel cancel_capture = { .stream = 1, .use_id = 3 };
	struct drm_capture_result capture_result;
	struct drm_capture_unregister_destination remove_destination = { .id = 1 };
	struct drm_capture_destroy_stream destroy_stream = { .id = 1 };
	struct drm_castkms_renderer_job *job;
	drmModeRes *resources;
	drmModeConnector *connector;
	drmModeModeInfo *mode;
	struct buffer source[2], private[2], output;
	struct monitor_control monitor;
	uint64_t host, worker, previous = 0, tiled_offer;
	unsigned int baseline;
	uint32_t plane;
	int fd, private_fd[2], output_fd, ready_timeline, ready_fence;
	void *fault;

	CHECK(argc == 2);
	fd = open(argv[1], O_RDWR | O_CLOEXEC);
	CHECK(fd >= 0 && drmIsMaster(fd));
	CHECK(drmSetClientCap(fd, DRM_CLIENT_CAP_ATOMIC, 1) == 0);
	CHECK(drmSetClientCap(fd, DRM_CLIENT_CAP_KMS_CONSTRAINTS, 1) == 0);
	resources = drmModeGetResources(fd);
	CHECK(resources && resources->count_crtcs > 0 && resources->count_connectors > 0);
	create.crtc_id = resources->crtcs[0];
	create.connector_id = resources->connectors[0];
	grant.crtc_id = create.crtc_id;
	grant.connector_id = create.connector_id;
	monitor = attach_fallback_monitor(fd, create.connector_id);
	host = selected(fd, create.crtc_id, 1);
	connector = drmModeGetConnector(fd, create.connector_id);
	CHECK(connector && connector->count_modes);
	mode = &connector->modes[0];
	plane = primary_plane(fd, 0);
	fault = mmap(NULL, 4096, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	CHECK(fault != MAP_FAILED);
	baseline = open_files();
	create.files = (uintptr_t)fault;
	expect_error(fd, DRM_IOCTL_CASTKMS_CREATE_RENDERER, &create, EFAULT);
	CHECK(open_files() == baseline);
	create.files = (uintptr_t)&files;
	CHECK(ioctl(fd, DRM_IOCTL_CASTKMS_CREATE_RENDERER, &create) == 0);
	CHECK(fcntl(files.renderer_fd, F_GETFD) == FD_CLOEXEC);
	CHECK(fcntl(files.revoke_fd, F_GETFD) == FD_CLOEXEC);
	CHECK(query_endpoint(files.renderer_fd).state == DRM_CASTKMS_RENDERER_STATE_EMPTY);
	readable(files.renderer_fd, 0);
	expect_error(files.renderer_fd, DRM_IOCTL_VERSION, &result, ENOTTY);
	configure.width = mode->hdisplay;
	configure.height = mode->vdisplay;
	image.width = mode->hdisplay;
	image.height = mode->vdisplay;
	constraints.header.min_output[0] = constraints.header.max_output[0] = mode->hdisplay;
	constraints.header.min_output[1] = constraints.header.max_output[1] = mode->vdisplay;
	memcpy(constraints.header.min_source, constraints.header.min_output,
		sizeof(constraints.header.min_source));
	memcpy(constraints.header.max_source, constraints.header.max_output,
		sizeof(constraints.header.max_source));
	configure.reserved[0] = 1;
	expect_error(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_CONFIGURE, &configure, EINVAL);
	configure.reserved[0] = 0;
	CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_CONFIGURE, &configure) == 0);
	CHECK(query_endpoint(files.renderer_fd).state == DRM_CASTKMS_RENDERER_STATE_CONFIGURED);
	expect_error(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_CONFIGURE,
		     &configure, EALREADY);
	expect_error(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_PUBLISH,
		     &publish, ENODATA);
	for (unsigned int i = 0; i < 2; i++) {
		private[i] = create_buffer(fd, mode->hdisplay, mode->vdisplay, 0);
		CHECK(drmPrimeHandleToFD(fd, private[i].dumb.handle, DRM_CLOEXEC | DRM_RDWR,
			&private_fd[i]) == 0);
		image.image_id = i + 1;
		image.buffers = (uintptr_t)&private_fd[i];
		CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_REGISTER_IMAGE,
			&image) == 0);
	}
	publish.result = (uintptr_t)fault;
	expect_error(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_PUBLISH, &publish, EFAULT);
	CHECK(selected(fd, create.crtc_id, 1) == host);
	CHECK(query_endpoint(files.renderer_fd).state == DRM_CASTKMS_RENDERER_STATE_CONFIGURED);
	ready_fence = pending_fence(&ready_timeline);
	memset(&result, 0xa5, sizeof(result));
	untouched = result;
	publish.result = (uintptr_t)&result;
	publish.ready_fence_fd = ready_fence;
	expect_error(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_PUBLISH, &publish, EBUSY);
	CHECK(!memcmp(&result, &untouched, sizeof(result)));
	CHECK(selected(fd, create.crtc_id, 1) == host);
	CHECK(query_endpoint(files.renderer_fd).state == DRM_CASTKMS_RENDERER_STATE_CONFIGURED);
	signal_fence(ready_timeline);
	CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_PUBLISH, &publish) == 0);
	CHECK(close(ready_fence) == 0);
	CHECK(close(ready_timeline) == 0);
	publish.ready_fence_fd = -1;
	worker = result.constraints_id;
	CHECK(worker && worker != host
		&& !result.reserved[0] && !result.reserved[1] && !result.reserved[2]);
	CHECK(query_endpoint(files.renderer_fd).constraints_id == worker);
	CHECK(selected(fd, create.crtc_id, 2) == host);
	readable(files.renderer_fd, 0);
	expect_error(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_PUBLISH,
		     &publish, EALREADY);
	expect_error(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_UNREGISTER_IMAGE,
		&remove, EBUSY);
	source[0] = create_buffer(fd, mode->hdisplay, mode->vdisplay, 0x33);
	source[1] = create_buffer(fd, mode->hdisplay, mode->vdisplay, 0x88);
	output = create_buffer(fd, mode->hdisplay, mode->vdisplay, 0);
	CHECK(drmPrimeHandleToFD(fd, output.dumb.handle, DRM_CLOEXEC | DRM_RDWR,
		&output_fd) == 0);
	CHECK(drmModeSetCrtc(fd, create.crtc_id, source[0].fb, 0, 0,
			     &create.connector_id, 1, mode) == 0);
	select_backend(fd, create.crtc_id, worker,
		       DRM_MODE_ATOMIC_ALLOW_MODESET | DRM_MODE_ATOMIC_TEST_ONLY);
	CHECK(selected(fd, create.crtc_id, 2) == host);
	select_backend(fd, create.crtc_id, worker, DRM_MODE_ATOMIC_ALLOW_MODESET);
	CHECK(selected(fd, create.crtc_id, 2) == worker);
	CHECK(ioctl(fd, DRM_IOCTL_MODE_CREATE_CAPTURE_GRANT, &grant) == 0);
	CHECK(fcntl(capture_files.capture_fd, F_GETFD) == FD_CLOEXEC);
	CHECK(fcntl(capture_files.control_fd, F_GETFD) == FD_CLOEXEC);
	description.format = DRM_FORMAT_ARGB8888;
	description.modifier = 9;
	CHECK(ioctl(capture_files.capture_fd, DRM_IOCTL_CAPTURE_DESCRIBE, &description) == 0);
	CHECK(description.format == DRM_FORMAT_ARGB8888 && description.modifier == 9);
	tiled_offer = description.id;
	stream.offer = tiled_offer;
	CHECK(ioctl(capture_files.capture_fd, DRM_IOCTL_CAPTURE_CREATE_STREAM, &stream) == 0);
	/* Exercise metadata and cancellation only; the dumb buffer is not tiled storage. */
	destination.format = DRM_FORMAT_ARGB8888;
	destination.modifier = 9;
	destination.width = mode->hdisplay;
	destination.height = mode->vdisplay;
	destination.fds[0] = output_fd;
	destination.strides[0] = output.dumb.pitch;
	destination.width--;
	expect_error(capture_files.capture_fd, DRM_IOCTL_CAPTURE_REGISTER_DESTINATION,
		     &destination, EINVAL);
	destination.width++;
	CHECK(ioctl(capture_files.capture_fd, DRM_IOCTL_CAPTURE_REGISTER_DESTINATION,
		&destination) == 0);
	CHECK(ioctl(capture_files.capture_fd, DRM_IOCTL_CAPTURE_QUEUE_OUTPUT, &queue) == 0);
	cancel_capture.use_id = queue.use_id;
	CHECK(ioctl(capture_files.capture_fd, DRM_IOCTL_CAPTURE_CANCEL,
		&cancel_capture) == 0);
	wait_capture(capture_files.capture_fd);
	take_capture.result = (uintptr_t)&capture_result;
	CHECK(ioctl(capture_files.capture_fd, DRM_IOCTL_CAPTURE_DEQUEUE, &take_capture) == 0);
	CHECK(capture_result.use_id == queue.use_id && capture_result.status == -ECANCELED);
	CHECK(ioctl(capture_files.capture_fd, DRM_IOCTL_CAPTURE_DESTROY_STREAM,
		&destroy_stream) == 0);
	CHECK(ioctl(capture_files.capture_fd, DRM_IOCTL_CAPTURE_UNREGISTER_DESTINATION,
		&remove_destination) == 0);
	description = (struct drm_capture_describe) {};
	CHECK(ioctl(capture_files.capture_fd, DRM_IOCTL_CAPTURE_DESCRIBE, &description) == 0);
	CHECK(description.id == tiled_offer + 1);
	CHECK(description.width == mode->hdisplay && description.height == mode->vdisplay);
	CHECK(description.format == DRM_FORMAT_XRGB8888);
	CHECK(description.modifier == DRM_FORMAT_MOD_LINEAR && description.max_requests);
	stream.id = 2;
	stream.offer = tiled_offer;
	expect_error(capture_files.capture_fd, DRM_IOCTL_CAPTURE_CREATE_STREAM,
		     &stream, ESTALE);
	stream.offer = description.id;
	CHECK(ioctl(capture_files.capture_fd, DRM_IOCTL_CAPTURE_CREATE_STREAM, &stream) == 0);
	destination.id = 2;
	destination.format = DRM_FORMAT_XRGB8888;
	destination.modifier = DRM_FORMAT_MOD_LINEAR;
	queue.stream = 2;
	queue.destination = 2;
	take_capture.stream = 2;
	cancel_capture.stream = 2;
	cancel_capture.use_id = 3;
	remove_destination.id = 2;
	destroy_stream.id = 2;
	destination.width = mode->hdisplay;
	destination.height = mode->vdisplay;
	destination.fds[0] = output_fd;
	destination.strides[0] = output.dumb.pitch;
	CHECK(ioctl(capture_files.capture_fd, DRM_IOCTL_CAPTURE_REGISTER_DESTINATION,
		&destination) == 0);
	CHECK(ioctl(capture_files.capture_fd, DRM_IOCTL_CAPTURE_QUEUE_OUTPUT, &queue) == 0);
	job = calloc(1, acquire.capacity);
	CHECK(job);
	acquire.result = (uintptr_t)fault;
	baseline = open_files();
	expect_error(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_ACQUIRE_JOB, &acquire, EFAULT);
	CHECK(open_files() == baseline);
	acquire.result = (uintptr_t)job;
	render_one(files.renderer_fd, &acquire, job, &release, worker, &previous,
		   private_fd[0], &private[0]);
	CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_RELEASE_JOB,
		    &release) == 0);

	/* Failed publication grants no access and leaks no DMA-BUF descriptor. */
	take_output.result = (uintptr_t)fault;
	baseline = open_files();
	expect_error(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_ACQUIRE_OUTPUT,
		     &take_output, EFAULT);
	CHECK(open_files() == baseline);
	wait_capture(capture_files.capture_fd);
	take_capture.result = (uintptr_t)&capture_result;
	memset(&capture_result, 0xa5, sizeof(capture_result));
	CHECK(ioctl(capture_files.capture_fd, DRM_IOCTL_CAPTURE_DEQUEUE, &take_capture) == 0);
	CHECK(capture_result.use_id == 1 && capture_result.status == -ECANCELED);
	CHECK(!capture_result.completed_at_ns && !capture_result.reserved);
	queue.use_id = 2;
	CHECK(ioctl(capture_files.capture_fd, DRM_IOCTL_CAPTURE_QUEUE_OUTPUT, &queue) == 0);
	take_output.result = (uintptr_t)&renderer_output;
	CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_ACQUIRE_OUTPUT,
		    &take_output) == 0);
	CHECK(renderer_output.image_id == 1 && renderer_output.width == mode->hdisplay);
	CHECK(renderer_output.height == mode->vdisplay);
	CHECK(renderer_output.format == DRM_FORMAT_XRGB8888 &&
	      renderer_output.memory_plane_count == 1);
	CHECK(renderer_output.modifier == DRM_FORMAT_MOD_LINEAR);
	CHECK(renderer_output.dma_buf_fd >= 0);
	CHECK(fcntl(renderer_output.dma_buf_fd, F_GETFD) == FD_CLOEXEC);
	CHECK(!renderer_output.reserved[0] && !renderer_output.reserved[1]);

	/* A held E-to-D claim must not retain A-to-E source access. */
	flip(fd, plane, source[1].fb);
	acquire.target_image_id = 2;
	render_one(files.renderer_fd, &acquire, job, &release, worker, &previous,
		   private_fd[1], &private[1]);
	copy_linear(private_fd[0], private[0].dumb.size, private[0].dumb.pitch, 0,
		    renderer_output.dma_buf_fd, output.dumb.size, renderer_output.pitch,
		    renderer_output.offset, mode->hdisplay, mode->vdisplay);
	release_output.job_id = renderer_output.job_id;
	CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_RELEASE_OUTPUT,
		&release_output) == 0);
	CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_RELEASE_OUTPUT,
		&release_output) == 0);
	CHECK(close(renderer_output.dma_buf_fd) == 0);
	wait_capture(capture_files.capture_fd);
	memset(&capture_result, 0xa5, sizeof(capture_result));
	CHECK(ioctl(capture_files.capture_fd, DRM_IOCTL_CAPTURE_DEQUEUE, &take_capture) == 0);
	CHECK(capture_result.use_id == 2 && capture_result.status == 0);
	CHECK(capture_result.completed_at_ns > 0 && !capture_result.reserved);
	check_pixels(output_fd, &output, mode->hdisplay, mode->vdisplay, 0x33);

	/* Cancellation does not revoke an output claim or permit early stream teardown. */
	queue.use_id = 3;
	CHECK(ioctl(capture_files.capture_fd, DRM_IOCTL_CAPTURE_QUEUE_OUTPUT, &queue) == 0);
	take_output.image_id = 2;
	CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_ACQUIRE_OUTPUT,
		    &take_output) == 0);
	CHECK(renderer_output.image_id == 2 && renderer_output.dma_buf_fd >= 0);
	CHECK(fcntl(renderer_output.dma_buf_fd, F_GETFD) == FD_CLOEXEC);
	CHECK(ioctl(capture_files.capture_fd, DRM_IOCTL_CAPTURE_CANCEL,
		&cancel_capture) == 0);
	expect_error(capture_files.capture_fd, DRM_IOCTL_CAPTURE_DESTROY_STREAM,
		&destroy_stream, EBUSY);
	copy_linear(private_fd[1], private[1].dumb.size, private[1].dumb.pitch, 0,
		    renderer_output.dma_buf_fd, output.dumb.size, renderer_output.pitch,
		    renderer_output.offset, mode->hdisplay, mode->vdisplay);
	release_output.job_id = renderer_output.job_id;
	CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_RELEASE_OUTPUT,
		&release_output) == 0);
	CHECK(close(renderer_output.dma_buf_fd) == 0);
	wait_capture(capture_files.capture_fd);
	memset(&capture_result, 0xa5, sizeof(capture_result));
	CHECK(ioctl(capture_files.capture_fd, DRM_IOCTL_CAPTURE_DEQUEUE, &take_capture) == 0);
	CHECK(capture_result.use_id == 3 && capture_result.status == -ECANCELED);
	CHECK(!capture_result.completed_at_ns && !capture_result.reserved);
	expect_error(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_ACQUIRE_OUTPUT,
		     &take_output, ENODATA);
	CHECK(ioctl(capture_files.capture_fd, DRM_IOCTL_CAPTURE_DESTROY_STREAM,
		&destroy_stream) == 0);

	for (unsigned int frame = 2; frame < 24; frame++) {
		flip(fd, plane, source[frame % 2].fb);
		acquire.target_image_id = frame % 2 + 1;
		render_one(files.renderer_fd, &acquire, job, &release, worker, &previous,
			   private_fd[frame % 2], &private[frame % 2]);
		expect_error(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_ACQUIRE_JOB,
			     &acquire, ENODATA);
	}
	stream.id = 3;
	CHECK(ioctl(capture_files.capture_fd, DRM_IOCTL_CAPTURE_CREATE_STREAM, &stream) == 0);
	queue.stream = 3;
	queue.use_id = 1;
	CHECK(ioctl(capture_files.capture_fd, DRM_IOCTL_CAPTURE_QUEUE_OUTPUT, &queue) == 0);
	flip(fd, plane, source[0].fb);
	acquire.target_image_id = 1;
	CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_ACQUIRE_JOB, &acquire) == 0);
	release.job_id = job->job_id;
	close_job(job);
	take_output.image_id = 2;
	CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_ACQUIRE_OUTPUT,
		    &take_output) == 0);
	CHECK(renderer_output.image_id == 2 && renderer_output.dma_buf_fd >= 0);
	CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_WITHDRAW, &withdraw) == 0);
	CHECK(query_endpoint(files.renderer_fd).state == DRM_CASTKMS_RENDERER_STATE_WITHDRAWN);
	hung_up(files.renderer_fd);
	expect_error(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_UNREGISTER_IMAGE,
		&remove, EBUSY);
	copy_linear(private_fd[1], private[1].dumb.size, private[1].dumb.pitch, 0,
		    renderer_output.dma_buf_fd, output.dumb.size, renderer_output.pitch,
		    renderer_output.offset, mode->hdisplay, mode->vdisplay);
	release_output.job_id = renderer_output.job_id;
	CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_RELEASE_OUTPUT,
		&release_output) == 0);
	CHECK(close(renderer_output.dma_buf_fd) == 0);
	wait_capture(capture_files.capture_fd);
	take_capture.stream = 3;
	memset(&capture_result, 0xa5, sizeof(capture_result));
	CHECK(ioctl(capture_files.capture_fd, DRM_IOCTL_CAPTURE_DEQUEUE, &take_capture) == 0);
	CHECK(capture_result.use_id == 1 && capture_result.status == -ECANCELED);
	CHECK(!capture_result.completed_at_ns && !capture_result.reserved);
	CHECK(ioctl(capture_files.capture_fd, DRM_IOCTL_CAPTURE_UNREGISTER_DESTINATION,
		&remove_destination) == 0);
	destroy_stream.id = 3;
	CHECK(ioctl(capture_files.capture_fd, DRM_IOCTL_CAPTURE_DESTROY_STREAM,
		&destroy_stream) == 0);
	CHECK(close(capture_files.control_fd) == 0);
	CHECK(close(capture_files.capture_fd) == 0);
	CHECK(close(files.revoke_fd) == 0);
	release.kind = DRM_CASTKMS_RENDERER_RELEASE_NO_ACCESS;
	CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_RELEASE_JOB, &release) == 0);
	for (unsigned int i = 0; i < 2; i++) {
		remove.image_id = i + 1;
		CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_UNREGISTER_IMAGE,
			&remove) == 0);
	}
	select_backend(fd, create.crtc_id, host, DRM_MODE_ATOMIC_ALLOW_MODESET);
	CHECK(close(files.renderer_fd) == 0);
	for (unsigned int i = 0; i < 2; i++)
		CHECK(close(private_fd[i]) == 0);
	CHECK(close(output_fd) == 0);
	CHECK(drmModeSetCrtc(fd, create.crtc_id, 0, 0, 0, NULL, 0, NULL) == 0);
	for (unsigned int i = 0; i < 2; i++)
		destroy_buffer(fd, &source[i]);
	for (unsigned int i = 0; i < 2; i++)
		destroy_buffer(fd, &private[i]);
	destroy_buffer(fd, &output);
	free(job);
	CHECK(munmap(fault, 4096) == 0);
	drmModeFreeConnector(connector);
	drmModeFreeResources(resources);
	close_monitor(&monitor);
	CHECK(close(fd) == 0);
	puts("PASS: renderer jobs, independent output delivery, 24 frames and cleanup");
	return 0;
}
