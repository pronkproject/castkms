// SPDX-License-Identifier: GPL-2.0-only

/* Immutable offers, ordinary atomic selection and source lifetimes. */
#include "fixture.h"

#include <dirent.h>
#include <drm_fourcc.h>
#include <fcntl.h>
#include <poll.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <unistd.h>

#include "../../../../include/uapi/drm/castkms_drm.h"
#include "../../../../include/uapi/drm/drm_constraints.h"

_Static_assert(sizeof(struct drm_castkms_renderer_query) == 32, "query layout");
_Static_assert(sizeof(struct drm_castkms_renderer_prepare_offer) == 48, "prepare layout");
_Static_assert(sizeof(struct drm_castkms_renderer_publish_offer) == 32, "publish layout");
_Static_assert(sizeof(struct drm_castkms_renderer_offer_result) == 32, "result layout");
_Static_assert(sizeof(struct drm_castkms_renderer_withdraw_offer) == 16, "withdraw layout");
_Static_assert(sizeof(struct drm_castkms_renderer_scene) == 56, "scene layout");

static void expect_error(int fd, unsigned long cmd, void *request, int error)
{
	errno = 0;
	CHECK(ioctl(fd, cmd, request) < 0);
	CHECK(errno == error);
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

static void select_offer(int fd, uint32_t crtc, uint64_t id, uint32_t flags)
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

static void close_scene(struct drm_castkms_renderer_scene *scene)
{
	char *cursor = (char *)(scene + 1);

	if (scene->producer_fd >= 0) {
		CHECK(fcntl(scene->producer_fd, F_GETFD) == FD_CLOEXEC);
		CHECK(close(scene->producer_fd) == 0);
	}
	for (uint32_t i = 0; i < scene->layer_count; i++) {
		struct drm_castkms_renderer_layer *layer = (void *)cursor;

		CHECK(layer->bytes >= sizeof(*layer) && layer->plane_count <= 4);
		for (uint32_t plane = 0; plane < layer->plane_count; plane++) {
			CHECK(fcntl(layer->planes[plane].dma_buf_fd, F_GETFD) == FD_CLOEXEC);
			CHECK(close(layer->planes[plane].dma_buf_fd) == 0);
		}
		cursor += layer->bytes;
		CHECK(cursor <= (char *)scene + scene->bytes);
	}
}

int main(int argc, char **argv)
{
	struct drm_castkms_renderer_files files = { .renderer_fd = -1, .revoke_fd = -1 };
	struct drm_castkms_create_renderer_control create = { .files = (uintptr_t)&files };
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
			.pitch_alignment = 1, .offset_alignment = 1, .max_pitch = 65536,
		},
	};
	struct drm_castkms_renderer_prepare_offer prepare = {
		.constraints = (uintptr_t)&constraints, .constraints_size = sizeof(constraints),
	};
	struct drm_castkms_renderer_submit_probe probe = { .completion_fd = -1 };
	struct drm_castkms_renderer_offer_result result;
	struct drm_castkms_renderer_publish_offer publish = { .result = (uintptr_t)&result };
	struct drm_castkms_renderer_withdraw_offer withdraw = { 0 };
	struct drm_castkms_renderer_register_image image = { .image_id = 1, .num_buffers = 1 };
	struct drm_castkms_renderer_unregister_image remove = { .image_id = 1 };
	struct drm_castkms_renderer_dequeue_scene dequeue = {
		.image_id = 1, .capacity = DRM_CASTKMS_RENDERER_SCENE_MAX_BYTES,
	};
	struct drm_castkms_renderer_release_source release = {
		.completion_fd = -1, .kind = DRM_CASTKMS_RENDERER_RELEASE_NO_ACCESS,
	};
	struct drm_castkms_renderer_scene *scene;
	drmModeRes *resources;
	drmModeConnector *connector;
	drmModeModeInfo *mode;
	struct buffer source[2], private;
	uint64_t host, worker, previous = 0;
	unsigned int baseline;
	uint32_t plane;
	int fd, private_fd;
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
	host = selected(fd, create.crtc_id, 1);
	connector = drmModeGetConnector(fd, create.connector_id);
	CHECK(connector && connector->count_modes);
	mode = &connector->modes[0];
	plane = primary_plane(fd, 0);
	fault = mmap(NULL, 4096, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	CHECK(fault != MAP_FAILED);
	baseline = open_files();
	create.files = (uintptr_t)fault;
	expect_error(fd, DRM_IOCTL_CASTKMS_CREATE_RENDERER_CONTROL, &create, EFAULT);
	CHECK(open_files() == baseline);
	create.files = (uintptr_t)&files;
	CHECK(ioctl(fd, DRM_IOCTL_CASTKMS_CREATE_RENDERER_CONTROL, &create) == 0);
	CHECK(fcntl(files.renderer_fd, F_GETFD) == FD_CLOEXEC);
	CHECK(fcntl(files.revoke_fd, F_GETFD) == FD_CLOEXEC);
	CHECK(query_endpoint(files.renderer_fd).state == DRM_CASTKMS_RENDERER_STATE_EMPTY);
	readable(files.renderer_fd, 0);
	expect_error(files.renderer_fd, DRM_IOCTL_VERSION, &result, ENOTTY);
	prepare.width = image.width = mode->hdisplay;
	prepare.height = image.height = mode->vdisplay;
	constraints.header.min_output[0] = constraints.header.max_output[0] = mode->hdisplay;
	constraints.header.min_output[1] = constraints.header.max_output[1] = mode->vdisplay;
	memcpy(constraints.header.min_source, constraints.header.min_output,
		sizeof(constraints.header.min_source));
	memcpy(constraints.header.max_source, constraints.header.max_output,
		sizeof(constraints.header.max_source));
	prepare.reserved[0] = 1;
	expect_error(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_PREPARE_OFFER, &prepare, EINVAL);
	prepare.reserved[0] = 0;
	CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_PREPARE_OFFER, &prepare) == 0);
	CHECK(query_endpoint(files.renderer_fd).state == DRM_CASTKMS_RENDERER_STATE_DRAFT);
	expect_error(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_PREPARE_OFFER,
		&prepare, EALREADY);
	expect_error(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_PUBLISH_OFFER,
		&publish, ENODATA);
	private = create_buffer(fd, mode->hdisplay, mode->vdisplay, 0);
	CHECK(drmPrimeHandleToFD(fd, private.dumb.handle, DRM_CLOEXEC | DRM_RDWR,
		&private_fd) == 0);
	image.buffers = (uintptr_t)&private_fd;
	CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_REGISTER_IMAGE, &image) == 0);
	CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_SUBMIT_PROBE, &probe) == 0);
	publish.result = (uintptr_t)fault;
	expect_error(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_PUBLISH_OFFER, &publish, EFAULT);
	CHECK(selected(fd, create.crtc_id, 1) == host);
	CHECK(query_endpoint(files.renderer_fd).state == DRM_CASTKMS_RENDERER_STATE_DRAFT);
	publish.result = (uintptr_t)&result;
	CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_PUBLISH_OFFER, &publish) == 0);
	worker = result.constraints_id;
	CHECK(worker && worker != host
		&& !result.reserved[0] && !result.reserved[1] && !result.reserved[2]);
	CHECK(query_endpoint(files.renderer_fd).constraints_id == worker);
	CHECK(selected(fd, create.crtc_id, 2) == host);
	readable(files.renderer_fd, 0);
	expect_error(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_PUBLISH_OFFER,
		&publish, EALREADY);
	expect_error(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_UNREGISTER_IMAGE,
		&remove, EBUSY);
	source[0] = create_buffer(fd, mode->hdisplay, mode->vdisplay, 0x33);
	source[1] = create_buffer(fd, mode->hdisplay, mode->vdisplay, 0x88);
	CHECK(drmModeSetCrtc(fd, create.crtc_id, source[0].fb, 0, 0,
			     &create.connector_id, 1, mode) == 0);
	select_offer(fd, create.crtc_id, worker,
		DRM_MODE_ATOMIC_ALLOW_MODESET | DRM_MODE_ATOMIC_TEST_ONLY);
	CHECK(selected(fd, create.crtc_id, 2) == host);
	select_offer(fd, create.crtc_id, worker, DRM_MODE_ATOMIC_ALLOW_MODESET);
	CHECK(selected(fd, create.crtc_id, 2) == worker);
	scene = calloc(1, dequeue.capacity);
	CHECK(scene);
	dequeue.result = (uintptr_t)fault;
	baseline = open_files();
	expect_error(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_DEQUEUE_SCENE, &dequeue, EFAULT);
	CHECK(open_files() == baseline);
	dequeue.result = (uintptr_t)scene;
	for (unsigned int frame = 0; frame < 24; frame++) {
		readable(files.renderer_fd, 1);
		CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_DEQUEUE_SCENE,
			&dequeue) == 0);
		CHECK(scene->version == DRM_CASTKMS_RENDERER_SCENE_VERSION);
		CHECK(scene->constraints_id == worker && scene->content_serial > previous);
		CHECK(scene->bytes == sizeof(*scene) + sizeof(struct drm_castkms_renderer_layer));
		CHECK(scene->layer_count == 1 && scene->producer_fd == -1
			&& !scene->output_color_count);
		previous = scene->content_serial;
		release.job_id = scene->job_id;
		expect_error(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_DEQUEUE_SCENE,
			&dequeue, EBUSY);
		readable(files.renderer_fd, 0);
		close_scene(scene);
		release.kind = DRM_CASTKMS_RENDERER_RELEASE_CPU_DONE;
		CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_RELEASE_SOURCE,
			&release) == 0);
		CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_RELEASE_SOURCE,
			&release) == 0);
		readable(files.renderer_fd, 0);
		expect_error(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_DEQUEUE_SCENE,
			&dequeue, ENODATA);
		flip(fd, plane, source[(frame + 1) % 2].fb);
	}
	CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_DEQUEUE_SCENE, &dequeue) == 0);
	release.job_id = scene->job_id;
	close_scene(scene);
	CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_WITHDRAW_OFFER, &withdraw) == 0);
	CHECK(query_endpoint(files.renderer_fd).state == DRM_CASTKMS_RENDERER_STATE_WITHDRAWN);
	expect_error(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_UNREGISTER_IMAGE,
		&remove, EBUSY);
	CHECK(close(files.revoke_fd) == 0);
	release.kind = DRM_CASTKMS_RENDERER_RELEASE_NO_ACCESS;
	CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_RELEASE_SOURCE, &release) == 0);
	CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_UNREGISTER_IMAGE, &remove) == 0);
	select_offer(fd, create.crtc_id, host, DRM_MODE_ATOMIC_ALLOW_MODESET);
	CHECK(close(files.renderer_fd) == 0);
	CHECK(close(private_fd) == 0);
	CHECK(drmModeSetCrtc(fd, create.crtc_id, 0, 0, 0, NULL, 0, NULL) == 0);
	for (unsigned int i = 0; i < 2; i++)
		destroy_buffer(fd, &source[i]);
	destroy_buffer(fd, &private);
	free(scene);
	CHECK(munmap(fault, 4096) == 0);
	drmModeFreeConnector(connector);
	drmModeFreeResources(resources);
	CHECK(close(fd) == 0);
	puts("PASS: immutable offers, atomic selection, 24 frames and revoked source release");
	return 0;
}
