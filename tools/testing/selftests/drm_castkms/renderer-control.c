// SPDX-License-Identifier: GPL-2.0-only

/* Renderer capability publication and revocation, without source access. */
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

_Static_assert(sizeof(struct drm_castkms_renderer_files) == 8,
	       "renderer file ABI");
_Static_assert(sizeof(struct drm_castkms_create_renderer_control) == 32,
	       "renderer creation ABI");
_Static_assert(sizeof(struct drm_castkms_renderer_query) == 24,
	       "renderer query ABI");
_Static_assert(sizeof(struct drm_castkms_renderer_takeover) == 40,
	       "renderer takeover ABI");
_Static_assert(sizeof(struct drm_castkms_renderer_begin_takeover) == 32,
	       "renderer begin ABI");
_Static_assert(sizeof(struct drm_castkms_renderer_abort_takeover) == 16,
	       "renderer abort ABI");
_Static_assert(sizeof(struct drm_castkms_renderer_snapshot) == 48,
	       "renderer snapshot ABI");
_Static_assert(sizeof(struct drm_castkms_renderer_get_snapshot) == 32,
	       "renderer snapshot request ABI");
_Static_assert(sizeof(struct drm_castkms_renderer_submit_probe) == 32,
	       "renderer probe submission ABI");
_Static_assert(sizeof(struct drm_castkms_renderer_commit_takeover) == 16,
	       "renderer takeover commit ABI");

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

static void expect_ioctl_error(int fd, unsigned long command, void *request,
			       int expected)
{
	errno = 0;
	CHECK(ioctl(fd, command, request) < 0);
	CHECK(errno == expected);
}

static struct drm_castkms_renderer_files create_renderer(
	int fd, const struct drm_castkms_create_renderer_control *template)
{
	struct drm_castkms_renderer_files files = { -1, -1 };
	struct drm_castkms_create_renderer_control request = *template;

	request.files = (uintptr_t)&files;
	CHECK(drmIoctl(fd, DRM_IOCTL_CASTKMS_CREATE_RENDERER_CONTROL,
		       &request) == 0);
	CHECK(files.renderer_fd >= 0 && files.revoke_fd >= 0);
	CHECK(files.renderer_fd != files.revoke_fd);
	return files;
}

static struct drm_castkms_renderer_query query_renderer_profile(int fd,
							 uint32_t profile)
{
	struct drm_castkms_renderer_query query;

	memset(&query, 0xa5, sizeof(query));
	CHECK(ioctl(fd, DRM_IOCTL_CASTKMS_RENDERER_QUERY, &query) == 0);
	CHECK(query.version == DRM_CASTKMS_RENDERER_VERSION);
	CHECK(query.flags == 0);
	CHECK(query.profile == profile);
	CHECK(query.reserved == 0);
	CHECK(query.generation != 0);
	return query;
}

static struct drm_castkms_renderer_query query_renderer(int fd)
{
	return query_renderer_profile(fd, DRM_CASTKMS_EXECUTION_HOST_V1);
}

static struct drm_castkms_renderer_takeover begin_takeover(int fd,
						   uint64_t generation)
{
	struct drm_castkms_renderer_takeover result;
	struct drm_castkms_renderer_begin_takeover request = {
		.expected_generation = generation,
		.result = (uintptr_t)&result,
	};

	memset(&result, 0xa5, sizeof(result));
	CHECK(ioctl(fd, DRM_IOCTL_CASTKMS_RENDERER_BEGIN_TAKEOVER,
		    &request) == 0);
	CHECK(result.candidate_id != 0);
	CHECK(result.execution_generation == generation);
	CHECK(result.profile == DRM_CASTKMS_EXECUTION_HOST_V1);
	CHECK(result.width != 0 && result.height != 0);
	CHECK(result.refresh_millihz != 0);
	CHECK(result.reserved == 0);
	return result;
}

static void abort_takeover(int fd, uint64_t candidate_id)
{
	struct drm_castkms_renderer_abort_takeover request = {
		.candidate_id = candidate_id,
	};

	CHECK(ioctl(fd, DRM_IOCTL_CASTKMS_RENDERER_ABORT_TAKEOVER,
		    &request) == 0);
}

static void submit_probe(int fd, uint64_t candidate_id, uint32_t source)
{
	struct drm_castkms_renderer_submit_probe request = {
		.candidate_id = candidate_id,
		.completion_fd = -1,
		.source = source,
	};

	CHECK(ioctl(fd, DRM_IOCTL_CASTKMS_RENDERER_SUBMIT_PROBE,
		    &request) == 0);
}

static void commit_takeover(int fd, uint64_t candidate_id)
{
	struct drm_castkms_renderer_commit_takeover request = {
		.candidate_id = candidate_id,
	};

	CHECK(ioctl(fd, DRM_IOCTL_CASTKMS_RENDERER_COMMIT_TAKEOVER,
		    &request) == 0);
}

static void retain_host_image(int master, uint32_t crtc, uint32_t connector,
			      const struct buffer *destination)
{
	struct drm_capture_grant_files files;
	struct drm_mode_create_capture_grant grant = {
		.crtc_id = crtc,
		.connector_id = connector,
		.files = (uintptr_t)&files,
	};
	struct drm_capture_describe description;
	struct drm_capture_create_stream stream = { .id = 1, .capacity = 1 };
	struct drm_capture_register_destination registration = {
		.id = 1,
		.width = destination->dumb.width,
		.height = destination->dumb.height,
		.format = DRM_FORMAT_XRGB8888,
		.num_planes = 1,
		.modifier = DRM_FORMAT_MOD_LINEAR,
		.strides = { destination->dumb.pitch },
	};
	struct drm_capture_queue_output request = {
		.stream = 1,
		.use_id = 1,
		.destination = 1,
		.reuse_fd = -1,
	};
	struct drm_capture_dequeue dequeue = { .stream = 1 };
	struct drm_capture_result result;
	struct pollfd event;
	int destination_fd;

	CHECK(drmPrimeHandleToFD(master, destination->dumb.handle,
				 DRM_CLOEXEC | DRM_RDWR, &destination_fd) == 0);
	registration.fds[0] = destination_fd;
	CHECK(ioctl(master, DRM_IOCTL_MODE_CREATE_CAPTURE_GRANT, &grant) == 0);
	CHECK(ioctl(files.capture_fd, DRM_IOCTL_CAPTURE_DESCRIBE,
		    &description) == 0);
	stream.offer = description.id;
	CHECK(ioctl(files.capture_fd, DRM_IOCTL_CAPTURE_CREATE_STREAM,
		    &stream) == 0);
	CHECK(ioctl(files.capture_fd, DRM_IOCTL_CAPTURE_REGISTER_DESTINATION,
		    &registration) == 0);
	CHECK(ioctl(files.capture_fd, DRM_IOCTL_CAPTURE_QUEUE_OUTPUT,
		    &request) == 0);
	event = (struct pollfd){ .fd = files.capture_fd, .events = POLLIN };
	CHECK(poll(&event, 1, 5000) == 1 && event.revents & POLLIN);
	dequeue.result = (uintptr_t)&result;
	CHECK(ioctl(files.capture_fd, DRM_IOCTL_CAPTURE_DEQUEUE, &dequeue) == 0);
	CHECK(result.use_id == 1 && result.status == 0);
	CHECK(close(files.control_fd) == 0);
	CHECK(close(files.capture_fd) == 0);
	CHECK(close(destination_fd) == 0);
}

static struct drm_castkms_renderer_snapshot get_snapshot(int fd,
							 uint64_t candidate_id)
{
	struct drm_castkms_renderer_snapshot result;
	struct drm_castkms_renderer_get_snapshot request = {
		.candidate_id = candidate_id,
		.result = (uintptr_t)&result,
	};

	memset(&result, 0xa5, sizeof(result));
	CHECK(ioctl(fd, DRM_IOCTL_CASTKMS_RENDERER_GET_SNAPSHOT, &request) == 0);
	CHECK(result.dma_buf_fd >= 0);
	CHECK(result.format == DRM_FORMAT_XRGB8888);
	CHECK(result.modifier == DRM_FORMAT_MOD_LINEAR);
	CHECK(result.width && result.height && result.pitch == result.width * 4);
	CHECK(!result.offset && result.content_serial && !result.flags && !result.reserved);
	return result;
}

static void check_snapshot(struct drm_castkms_renderer_snapshot *snapshot,
			   unsigned char value)
{
	struct dma_buf_sync sync = { .flags = DMA_BUF_SYNC_START | DMA_BUF_SYNC_READ };
	size_t size = (size_t)snapshot->pitch * snapshot->height;
	unsigned char *pixels;
	void *writable;

	CHECK(fcntl(snapshot->dma_buf_fd, F_GETFD) == FD_CLOEXEC);
	CHECK((fcntl(snapshot->dma_buf_fd, F_GETFL) & O_ACCMODE) == O_RDONLY);
	errno = 0;
	writable = mmap(NULL, size, PROT_WRITE, MAP_SHARED,
			snapshot->dma_buf_fd, 0);
	CHECK(writable == MAP_FAILED && errno == EACCES);
	pixels = mmap(NULL, size, PROT_READ, MAP_SHARED, snapshot->dma_buf_fd, 0);
	CHECK(pixels != MAP_FAILED);
	CHECK(ioctl(snapshot->dma_buf_fd, DMA_BUF_IOCTL_SYNC, &sync) == 0);
	for (uint32_t y = 0; y < snapshot->height; y++) {
		for (uint32_t x = 0; x < snapshot->width; x++) {
			for (uint32_t channel = 0; channel < 3; channel++)
				CHECK(pixels[y * snapshot->pitch + x * 4 + channel] == value);
		}
	}
	sync.flags = DMA_BUF_SYNC_END | DMA_BUF_SYNC_READ;
	CHECK(ioctl(snapshot->dma_buf_fd, DMA_BUF_IOCTL_SYNC, &sync) == 0);
	CHECK(munmap(pixels, size) == 0);
}

int main(int argc, char **argv)
{
	struct drm_castkms_create_renderer_control request = {};
	struct drm_castkms_renderer_files files;
	struct drm_castkms_renderer_files next_files;
	struct drm_castkms_renderer_query first, next;
	struct drm_castkms_renderer_takeover candidate, replacement;
	struct drm_castkms_renderer_begin_takeover begin = {};
	struct drm_castkms_renderer_abort_takeover abort = {};
	struct drm_castkms_renderer_get_snapshot snapshot_request = {};
	struct drm_castkms_renderer_submit_probe probe = { .completion_fd = -1 };
	struct drm_castkms_renderer_commit_takeover commit = {};
	struct drm_castkms_renderer_snapshot snapshot;
	struct drm_castkms_renderer_snapshot snapshot_duplicate;
	drmModeConnector *connector;
	drmModeRes *resources;
	struct buffer buffer, capture_output;
	uint32_t connector_id;
	unsigned int before;
	int duplicate, fd, peer;

	if (argc != 2) {
		fprintf(stderr, "SKIP: supply a disposable Rust CastKMS DRM node\n");
		return 4;
	}
	fd = open(argv[1], O_RDWR | O_CLOEXEC);
	CHECK(fd >= 0);
	CHECK(drmSetMaster(fd) == 0);
	peer = open(argv[1], O_RDWR | O_CLOEXEC);
	CHECK(peer >= 0 && !drmIsMaster(peer));
	resources = drmModeGetResources(fd);
	CHECK(resources && resources->count_crtcs == 1 &&
	      resources->count_connectors == 1);
	request.crtc_id = resources->crtcs[0];
	request.connector_id = resources->connectors[0];
	connector_id = request.connector_id;
	drmModeFreeResources(resources);

	request.files = 1;
	expect_ioctl_error(fd, DRM_IOCTL_CASTKMS_CREATE_RENDERER_CONTROL, &request,
			   ENODEV);
	connector = drmModeGetConnector(fd, connector_id);
	CHECK(connector && connector->count_modes > 0);
	buffer = create_buffer(fd, connector->modes[0].hdisplay,
			       connector->modes[0].vdisplay, 0x57);
	CHECK(drmModeSetCrtc(fd, request.crtc_id, buffer.fb, 0, 0,
			     &connector_id, 1, &connector->modes[0]) == 0);
	capture_output = create_buffer(fd, buffer.dumb.width, buffer.dumb.height,
				       0x19);
	drmModeFreeConnector(connector);
	files = create_renderer(fd, &request);
	first = query_renderer(files.renderer_fd);
	candidate = begin_takeover(files.renderer_fd, first.generation);
	snapshot_request.candidate_id = candidate.candidate_id;
	snapshot_request.result = (uintptr_t)&snapshot;
	expect_ioctl_error(files.renderer_fd,
			   DRM_IOCTL_CASTKMS_RENDERER_GET_SNAPSHOT,
			   &snapshot_request, ENODATA);
	probe.candidate_id = candidate.candidate_id;
	probe.source = DRM_CASTKMS_RENDERER_PROBE_STARTUP_IMAGE;
	expect_ioctl_error(files.renderer_fd,
			   DRM_IOCTL_CASTKMS_RENDERER_SUBMIT_PROBE,
			   &probe, ENODATA);
	submit_probe(files.renderer_fd, candidate.candidate_id,
		     DRM_CASTKMS_RENDERER_PROBE_PRIVATE);
	expect_ioctl_error(files.renderer_fd,
			   DRM_IOCTL_CASTKMS_RENDERER_SUBMIT_PROBE,
			   &probe, EALREADY);
	abort_takeover(files.renderer_fd, candidate.candidate_id);
	CHECK(close(files.renderer_fd) == 0);
	CHECK(close(files.revoke_fd) == 0);
	retain_host_image(fd, request.crtc_id, request.connector_id,
			  &capture_output);

	before = open_files();
	for (unsigned int i = 0; i < 32; i++)
		expect_ioctl_error(fd, DRM_IOCTL_CASTKMS_CREATE_RENDERER_CONTROL,
				   &request, EFAULT);
	CHECK(open_files() == before);
	request.files = (uintptr_t)&files;
	request.flags = 1;
	expect_ioctl_error(fd, DRM_IOCTL_CASTKMS_CREATE_RENDERER_CONTROL, &request,
			   EINVAL);
	request.flags = 0;
	for (unsigned int i = 0; i < 3; i++) {
		request.reserved[i] = 1;
		expect_ioctl_error(fd, DRM_IOCTL_CASTKMS_CREATE_RENDERER_CONTROL,
				   &request, EINVAL);
		request.reserved[i] = 0;
	}
	expect_ioctl_error(peer, DRM_IOCTL_CASTKMS_CREATE_RENDERER_CONTROL, &request,
			   EACCES);
	request.crtc_id = 0;
	expect_ioctl_error(fd, DRM_IOCTL_CASTKMS_CREATE_RENDERER_CONTROL, &request,
			   EINVAL);
	request.crtc_id = request.connector_id;
	expect_ioctl_error(fd, DRM_IOCTL_CASTKMS_CREATE_RENDERER_CONTROL, &request,
			   ENOENT);
	resources = drmModeGetResources(fd);
	CHECK(resources && resources->count_crtcs == 1);
	request.crtc_id = resources->crtcs[0];
	drmModeFreeResources(resources);

	files = create_renderer(fd, &request);
	CHECK(fcntl(files.renderer_fd, F_GETFD) == FD_CLOEXEC);
	CHECK(fcntl(files.revoke_fd, F_GETFD) == FD_CLOEXEC);
	CHECK((fcntl(files.renderer_fd, F_GETFL) & O_ACCMODE) == O_RDWR);
	expect_ioctl_error(files.renderer_fd, _IO('x', 0xff), NULL, ENOTTY);
	expect_ioctl_error(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_QUERY,
			   NULL, EFAULT);
	first = query_renderer(files.renderer_fd);
	expect_ioctl_error(files.revoke_fd, DRM_IOCTL_CASTKMS_RENDERER_QUERY,
			   &next, ENOTTY);
	expect_ioctl_error(files.renderer_fd,
			   DRM_IOCTL_CASTKMS_RENDERER_QUERY ^
			   (1U << _IOC_SIZESHIFT), &next, ENOTTY);
	begin.expected_generation = first.generation + 1;
	begin.result = (uintptr_t)&candidate;
	expect_ioctl_error(files.renderer_fd,
			   DRM_IOCTL_CASTKMS_RENDERER_BEGIN_TAKEOVER,
			   &begin, ESTALE);
	begin.expected_generation = first.generation;
	begin.result = 1;
	expect_ioctl_error(files.renderer_fd,
			   DRM_IOCTL_CASTKMS_RENDERER_BEGIN_TAKEOVER,
			   &begin, EFAULT);
	candidate = begin_takeover(files.renderer_fd, first.generation);
	snapshot_request.result = (uintptr_t)&snapshot;
	snapshot_request.candidate_id = 0;
	expect_ioctl_error(files.renderer_fd,
			   DRM_IOCTL_CASTKMS_RENDERER_GET_SNAPSHOT,
			   &snapshot_request, EINVAL);
	snapshot_request.candidate_id = candidate.candidate_id + 1;
	expect_ioctl_error(files.renderer_fd,
			   DRM_IOCTL_CASTKMS_RENDERER_GET_SNAPSHOT,
			   &snapshot_request, ENOENT);
	snapshot_request.candidate_id = candidate.candidate_id;
	snapshot_request.result = 0;
	expect_ioctl_error(files.renderer_fd,
			   DRM_IOCTL_CASTKMS_RENDERER_GET_SNAPSHOT,
			   &snapshot_request, EINVAL);
	snapshot_request.result = (uintptr_t)&snapshot;
	snapshot_request.flags = 1;
	expect_ioctl_error(files.renderer_fd,
			   DRM_IOCTL_CASTKMS_RENDERER_GET_SNAPSHOT,
			   &snapshot_request, EINVAL);
	snapshot_request.flags = 0;
	for (unsigned int i = 0; i < 3; i++) {
		snapshot_request.reserved[i] = 1;
		expect_ioctl_error(files.renderer_fd,
				   DRM_IOCTL_CASTKMS_RENDERER_GET_SNAPSHOT,
				   &snapshot_request, EINVAL);
		snapshot_request.reserved[i] = 0;
	}
	before = open_files();
	snapshot_request.result = 1;
	for (unsigned int i = 0; i < 8; i++)
		expect_ioctl_error(files.renderer_fd,
				   DRM_IOCTL_CASTKMS_RENDERER_GET_SNAPSHOT,
				   &snapshot_request, EFAULT);
	CHECK(open_files() == before);
	snapshot = get_snapshot(files.renderer_fd, candidate.candidate_id);
	snapshot_request.result = (uintptr_t)&snapshot_duplicate;
	before = open_files();
	expect_ioctl_error(files.renderer_fd,
			   DRM_IOCTL_CASTKMS_RENDERER_GET_SNAPSHOT,
			   &snapshot_request, EALREADY);
	CHECK(open_files() == before);
	probe.candidate_id = 0;
	expect_ioctl_error(files.renderer_fd,
			   DRM_IOCTL_CASTKMS_RENDERER_SUBMIT_PROBE,
			   &probe, EINVAL);
	probe.candidate_id = candidate.candidate_id + 1;
	expect_ioctl_error(files.renderer_fd,
			   DRM_IOCTL_CASTKMS_RENDERER_SUBMIT_PROBE,
			   &probe, ENOENT);
	probe.candidate_id = candidate.candidate_id;
	probe.source = 0;
	probe.completion_fd = 123456;
	expect_ioctl_error(files.renderer_fd,
			   DRM_IOCTL_CASTKMS_RENDERER_SUBMIT_PROBE,
			   &probe, EINVAL);
	probe.source = DRM_CASTKMS_RENDERER_PROBE_STARTUP_IMAGE;
	probe.completion_fd = -2;
	expect_ioctl_error(files.renderer_fd,
			   DRM_IOCTL_CASTKMS_RENDERER_SUBMIT_PROBE,
			   &probe, EINVAL);
	probe.completion_fd = fd;
	expect_ioctl_error(files.renderer_fd,
			   DRM_IOCTL_CASTKMS_RENDERER_SUBMIT_PROBE,
			   &probe, EINVAL);
	probe.completion_fd = -1;
	probe.flags = 1;
	expect_ioctl_error(files.renderer_fd,
			   DRM_IOCTL_CASTKMS_RENDERER_SUBMIT_PROBE,
			   &probe, EINVAL);
	probe.flags = 0;
	for (unsigned int i = 0; i < 3; i++) {
		probe.reserved[i] = 1;
		expect_ioctl_error(files.renderer_fd,
				   DRM_IOCTL_CASTKMS_RENDERER_SUBMIT_PROBE,
				   &probe, EINVAL);
		probe.reserved[i] = 0;
	}
	submit_probe(files.renderer_fd, candidate.candidate_id,
		     DRM_CASTKMS_RENDERER_PROBE_STARTUP_IMAGE);
	expect_ioctl_error(files.renderer_fd,
			   DRM_IOCTL_CASTKMS_RENDERER_SUBMIT_PROBE,
			   &probe, EALREADY);
	expect_ioctl_error(files.renderer_fd,
			   DRM_IOCTL_CASTKMS_RENDERER_BEGIN_TAKEOVER,
			   &begin, EBUSY);
	commit.candidate_id = 0;
	expect_ioctl_error(files.renderer_fd,
			   DRM_IOCTL_CASTKMS_RENDERER_COMMIT_TAKEOVER,
			   &commit, EINVAL);
	commit.candidate_id = candidate.candidate_id + 1;
	expect_ioctl_error(files.renderer_fd,
			   DRM_IOCTL_CASTKMS_RENDERER_COMMIT_TAKEOVER,
			   &commit, ENOENT);
	commit.candidate_id = candidate.candidate_id;
	commit.flags = 1;
	expect_ioctl_error(files.renderer_fd,
			   DRM_IOCTL_CASTKMS_RENDERER_COMMIT_TAKEOVER,
			   &commit, EINVAL);
	commit.flags = 0;
	commit.reserved = 1;
	expect_ioctl_error(files.renderer_fd,
			   DRM_IOCTL_CASTKMS_RENDERER_COMMIT_TAKEOVER,
			   &commit, EINVAL);
	commit.reserved = 0;
	abort.candidate_id = candidate.candidate_id + 1;
	expect_ioctl_error(files.renderer_fd,
			   DRM_IOCTL_CASTKMS_RENDERER_ABORT_TAKEOVER,
			   &abort, ENOENT);
	abort.candidate_id = candidate.candidate_id;
	abort.flags = 1;
	expect_ioctl_error(files.renderer_fd,
			   DRM_IOCTL_CASTKMS_RENDERER_ABORT_TAKEOVER,
			   &abort, EINVAL);
	abort.flags = 0;
	abort_takeover(files.renderer_fd, candidate.candidate_id);
	probe.candidate_id = candidate.candidate_id;
	expect_ioctl_error(files.renderer_fd,
			   DRM_IOCTL_CASTKMS_RENDERER_SUBMIT_PROBE,
			   &probe, ENOENT);
	snapshot_request.result = (uintptr_t)&snapshot;
	expect_ioctl_error(files.renderer_fd,
			   DRM_IOCTL_CASTKMS_RENDERER_GET_SNAPSHOT,
			   &snapshot_request, ENOENT);
	replacement = begin_takeover(files.renderer_fd, first.generation);
	CHECK(replacement.candidate_id > candidate.candidate_id);

	duplicate = fcntl(files.revoke_fd, F_DUPFD_CLOEXEC, 0);
	CHECK(duplicate >= 0);
	CHECK(close(files.revoke_fd) == 0);
	next = query_renderer(files.renderer_fd);
	CHECK(!memcmp(&first, &next, sizeof(first)));
	CHECK(close(duplicate) == 0);
	expect_ioctl_error(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_QUERY,
			   &next, EKEYREVOKED);
	probe.candidate_id = replacement.candidate_id;
	expect_ioctl_error(files.renderer_fd,
			   DRM_IOCTL_CASTKMS_RENDERER_SUBMIT_PROBE,
			   &probe, EKEYREVOKED);
	CHECK(close(files.renderer_fd) == 0);
	check_snapshot(&snapshot, 0x57);
	CHECK(close(snapshot.dma_buf_fd) == 0);

	files = create_renderer(fd, &request);
	first = query_renderer(files.renderer_fd);
	candidate = begin_takeover(files.renderer_fd, first.generation);
	CHECK(candidate.candidate_id != 0);
	duplicate = fcntl(files.renderer_fd, F_DUPFD_CLOEXEC, 0);
	CHECK(duplicate >= 0);
	CHECK(close(files.renderer_fd) == 0);
	query_renderer(duplicate);
	CHECK(drmDropMaster(fd) == 0);
	CHECK(drmSetMaster(peer) == 0);
	expect_ioctl_error(duplicate, DRM_IOCTL_CASTKMS_RENDERER_QUERY, &next,
			   EACCES);
	CHECK(close(files.revoke_fd) == 0);
	CHECK(close(duplicate) == 0);

	request.files = (uintptr_t)&files;
	files = create_renderer(peer, &request);
	first = query_renderer(files.renderer_fd);
	candidate = begin_takeover(files.renderer_fd, first.generation);
	CHECK(close(files.renderer_fd) == 0);
	next_files = create_renderer(peer, &request);
	candidate = begin_takeover(next_files.renderer_fd, first.generation);
	submit_probe(next_files.renderer_fd, candidate.candidate_id,
		     DRM_CASTKMS_RENDERER_PROBE_PRIVATE);
	commit_takeover(next_files.renderer_fd, candidate.candidate_id);
	commit_takeover(next_files.renderer_fd, candidate.candidate_id);
	next = query_renderer_profile(next_files.renderer_fd,
				      DRM_CASTKMS_EXECUTION_GPU_V1);
	CHECK(next.generation == first.generation + 1);
	abort.candidate_id = candidate.candidate_id;
	expect_ioctl_error(next_files.renderer_fd,
			   DRM_IOCTL_CASTKMS_RENDERER_ABORT_TAKEOVER,
			   &abort, EALREADY);
	begin.expected_generation = next.generation;
	expect_ioctl_error(next_files.renderer_fd,
			   DRM_IOCTL_CASTKMS_RENDERER_BEGIN_TAKEOVER,
			   &begin, EBUSY);
	CHECK(close(next_files.renderer_fd) == 0);
	CHECK(close(next_files.revoke_fd) == 0);
	CHECK(close(files.revoke_fd) == 0);
	destroy_buffer(fd, &buffer);
	destroy_buffer(fd, &capture_output);
	CHECK(close(peer) == 0);
	CHECK(close(fd) == 0);
	puts("PASS: renderer capability publication, query and revocation");
	return 0;
}
