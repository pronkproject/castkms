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
#include "../../../../include/uapi/drm/drm_prepare.h"

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
_Static_assert(sizeof(struct drm_castkms_renderer_source_plane) == 16,
	       "renderer source plane ABI");
_Static_assert(sizeof(struct drm_castkms_renderer_release_source) == 32,
	       "renderer source release ABI");
_Static_assert(sizeof(struct drm_castkms_renderer_dequeue_scene) == 32,
	       "renderer scene request ABI");
_Static_assert(sizeof(struct drm_castkms_renderer_register_image) == 48,
	       "renderer private image registration ABI");
_Static_assert(sizeof(struct drm_castkms_renderer_unregister_image) == 16,
	       "renderer private image removal ABI");

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

static void check_transition_property(int fd, uint32_t crtc)
{
	drmModeAtomicReq *update = drmModeAtomicAlloc();
	drmModeObjectProperties *properties;
	unsigned int found = 0;

	CHECK(update);
	property(fd, update, crtc, DRM_MODE_OBJECT_CRTC, DRM_CASTKMS_TRANSITION_PROPERTY, 0);
	CHECK(drmModeAtomicCommit(fd, update, DRM_MODE_ATOMIC_TEST_ONLY, NULL) == 0);
	CHECK(drmModeAtomicCommit(fd, update, 0, NULL) == 0);
	drmModeAtomicFree(update);
	update = drmModeAtomicAlloc();
	CHECK(update);
	property(fd, update, crtc, DRM_MODE_OBJECT_CRTC, DRM_CASTKMS_TRANSITION_PROPERTY, UINT64_MAX);
	errno = 0;
	CHECK(drmModeAtomicCommit(fd, update, DRM_MODE_ATOMIC_TEST_ONLY, NULL) < 0);
	CHECK(errno == ESTALE);
	errno = 0;
	CHECK(drmModeAtomicCommit(fd, update, 0, NULL) < 0);
	CHECK(errno == ESTALE);
	drmModeAtomicFree(update);
	properties = drmModeObjectGetProperties(fd, crtc, DRM_MODE_OBJECT_CRTC);
	CHECK(properties);
	for (uint32_t i = 0; i < properties->count_props; i++) {
		drmModePropertyRes *description = drmModeGetProperty(fd, properties->props[i]);

		CHECK(description);
		if (!strcmp(description->name, DRM_CASTKMS_TRANSITION_PROPERTY)) {
			CHECK(properties->prop_values[i] == 0);
			CHECK(description->flags & DRM_MODE_PROP_ATOMIC);
			found++;
		}
		drmModeFreeProperty(description);
	}
	CHECK(found == 1);
	drmModeFreeObjectProperties(properties);
}

static void check_plane_blending(int fd)
{
	drmModePlaneRes *planes = drmModeGetPlaneResources(fd);

	CHECK(planes && planes->count_planes);
	for (uint32_t plane = 0; plane < planes->count_planes; plane++) {
		drmModeObjectProperties *properties = drmModeObjectGetProperties(
			fd, planes->planes[plane], DRM_MODE_OBJECT_PLANE);
		unsigned int found = 0;

		CHECK(properties);
		for (uint32_t i = 0; i < properties->count_props; i++) {
			drmModePropertyRes *description = drmModeGetProperty(
				fd, properties->props[i]);

			CHECK(description);
			if (!strcmp(description->name, "pixel blend mode")) {
				CHECK(description->flags & DRM_MODE_PROP_ENUM);
				CHECK(description->count_enums == 1);
				CHECK(!strcmp(description->enums[0].name,
					      "Pre-multiplied"));
				CHECK(properties->prop_values[i] ==
				      description->enums[0].value);
				found++;
			}
			drmModeFreeProperty(description);
		}
		CHECK(found == 1);
		drmModeFreeObjectProperties(properties);
	}
	drmModeFreePlaneResources(planes);
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
	CHECK(result.profile == DRM_CASTKMS_EXECUTION_HOST_V1 ||
	      result.profile == DRM_CASTKMS_EXECUTION_GPU_V1);
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

static struct drm_castkms_renderer_capabilities query_capabilities(int fd)
{
	void *bytes = calloc(1, DRM_CASTKMS_CAPABILITY_QUERY_MAX_BYTES);
	struct drm_castkms_renderer_query_capabilities request = {
		.result = (uintptr_t)bytes,
		.capacity = sizeof(struct drm_castkms_renderer_capabilities),
	};
	struct drm_castkms_renderer_capabilities result;

	CHECK(bytes);
	expect_ioctl_error(fd, DRM_IOCTL_CASTKMS_RENDERER_QUERY_CAPABILITIES,
			   &request, ENOSPC);
	memcpy(&result, bytes, sizeof(result));
	CHECK(result.size > sizeof(result));
	CHECK(result.size <= DRM_CASTKMS_CAPABILITY_QUERY_MAX_BYTES);
	request.capacity = DRM_CASTKMS_CAPABILITY_QUERY_MAX_BYTES;
	CHECK(ioctl(fd, DRM_IOCTL_CASTKMS_RENDERER_QUERY_CAPABILITIES, &request) == 0);
	memcpy(&result, bytes, sizeof(result));
	CHECK(result.version == DRM_CASTKMS_CAPABILITY_VERSION);
	CHECK(result.active_offset == sizeof(result));
	CHECK(result.active_size >= sizeof(struct drm_castkms_capability_profile));
	CHECK(result.active_offset + result.active_size <= result.size);
	if (result.flags & DRM_CASTKMS_CAPABILITY_STATE_PENDING) {
		CHECK(result.pending_offset == result.active_offset + result.active_size);
		CHECK(result.pending_offset + result.pending_size == result.size);
		CHECK(result.transition && result.pending_generation);
	} else {
		CHECK(!result.pending_offset && !result.pending_size);
		CHECK(!result.transition && !result.pending_generation);
	}
	free(bytes);
	return result;
}

static void tag_transition(int fd, uint32_t crtc, uint64_t token, bool test_only)
{
	drmModeAtomicReq *update = drmModeAtomicAlloc();

	CHECK(update);
	property(fd, update, crtc, DRM_MODE_OBJECT_CRTC,
		 DRM_CASTKMS_TRANSITION_PROPERTY, token);
	CHECK(drmModeAtomicCommit(fd, update,
				 test_only ? DRM_MODE_ATOMIC_TEST_ONLY : 0, NULL) == 0);
	drmModeAtomicFree(update);
}

static uint32_t overlay_plane(int fd, uint32_t crtc)
{
	drmModeRes *resources = drmModeGetResources(fd);
	drmModePlaneRes *planes = drmModeGetPlaneResources(fd);
	uint32_t result = 0, mask = 0;

	CHECK(resources && planes);
	for (int i = 0; i < resources->count_crtcs; i++)
		if (resources->crtcs[i] == crtc)
			mask = 1U << i;
	for (uint32_t i = 0; i < planes->count_planes && !result; i++) {
		drmModePlane *plane = drmModeGetPlane(fd, planes->planes[i]);
		CHECK(plane);
		if (plane->possible_crtcs & mask) {
			drmModeObjectProperties *props = drmModeObjectGetProperties(fd,
				plane->plane_id, DRM_MODE_OBJECT_PLANE);
			CHECK(props);
			for (uint32_t p = 0; p < props->count_props; p++) {
				drmModePropertyRes *prop = drmModeGetProperty(fd, props->props[p]);
				CHECK(prop);
				if (!strcmp(prop->name, "type") &&
				    props->prop_values[p] == DRM_PLANE_TYPE_OVERLAY)
					result = plane->plane_id;
				drmModeFreeProperty(prop);
			}
			drmModeFreeObjectProperties(props);
		}
		drmModeFreePlane(plane);
	}
	drmModeFreePlaneResources(planes);
	drmModeFreeResources(resources);
	CHECK(result);
	return result;
}

static uint64_t register_linear_profile(int fd, uint64_t candidate,
				       uint32_t width, uint32_t height)
{
	struct {
		struct drm_castkms_capability_profile header;
		struct drm_castkms_capability_format formats[2];
	} profile = {
		.header = {
			.version = DRM_CASTKMS_CAPABILITY_VERSION,
			.kind = DRM_CASTKMS_CAPABILITY_KIND_RENDERER,
			.flags = DRM_CASTKMS_CAPABILITY_PROFILE_CROP | DRM_CASTKMS_CAPABILITY_PROFILE_FRACTIONAL |
				 DRM_CASTKMS_CAPABILITY_PROFILE_POSITION | DRM_CASTKMS_CAPABILITY_PROFILE_SCALE |
				 DRM_CASTKMS_CAPABILITY_PROFILE_SRGB | DRM_CASTKMS_CAPABILITY_PROFILE_PLANE_MATRIX |
				 DRM_CASTKMS_CAPABILITY_PROFILE_OUTPUT_MATRIX,
			.format_count = 2, .max_output = { width, height },
			.min_output = { width, height }, .min_source = { 1, 1 },
			.max_source = { 16384, 16384 }, .min_scale = 1 << 12,
			.max_scale = 1 << 20, .max_layers = 24, .max_roles = { 1, 22, 1 },
			.max_color_operations = 16, .max_lut_entries = 256,
			.yuv_encodings = DRM_CASTKMS_CAPABILITY_YUV_ENCODING_BT601 |
				DRM_CASTKMS_CAPABILITY_YUV_ENCODING_BT709 |
				DRM_CASTKMS_CAPABILITY_YUV_ENCODING_BT2020,
			.yuv_ranges = DRM_CASTKMS_CAPABILITY_YUV_RANGE_LIMITED |
				DRM_CASTKMS_CAPABILITY_YUV_RANGE_FULL,
		},
	};
	struct drm_castkms_renderer_profile_result result;
	struct drm_castkms_renderer_register_profile request = {
		.candidate_id = candidate, .profile = (uintptr_t)&profile,
		.result = (uintptr_t)&result, .profile_size = sizeof(profile),
	};
	struct drm_castkms_renderer_capabilities state;

	for (unsigned int i = 0; i < 2; i++) {
		profile.formats[i] = (struct drm_castkms_capability_format) {
			.fourcc = DRM_FORMAT_XRGB8888, .plane_count = 1,
			.flags = DRM_CASTKMS_CAPABILITY_FORMAT_NATIVE | DRM_CASTKMS_CAPABILITY_FORMAT_IMPORTED |
				 (i ? DRM_CASTKMS_CAPABILITY_FORMAT_EXPLICIT_MODIFIER : 0),
			.pitch_alignment = 1, .offset_alignment = 1, .max_pitch = UINT32_MAX,
		};
	}
	for (unsigned int i = 0; i < 2; i++) {
		request.reserved[i] = 1;
		expect_ioctl_error(fd, DRM_IOCTL_CASTKMS_RENDERER_REGISTER_PROFILE, &request, EINVAL);
		request.reserved[i] = 0;
	}
	_Static_assert(sizeof(request) == 48, "renderer registration layout");
	profile.header.reserved[0] = 1;
	expect_ioctl_error(fd, DRM_IOCTL_CASTKMS_RENDERER_REGISTER_PROFILE, &request, EINVAL);
	profile.header.reserved[0] = 0;
	profile.header.min_output[0] = 0;
	expect_ioctl_error(fd, DRM_IOCTL_CASTKMS_RENDERER_REGISTER_PROFILE, &request, EINVAL);
	profile.header.min_output[0] = width + 1;
	expect_ioctl_error(fd, DRM_IOCTL_CASTKMS_RENDERER_REGISTER_PROFILE, &request, EINVAL);
	profile.header.min_output[0] = width;
	CHECK(!(query_capabilities(fd).flags & DRM_CASTKMS_CAPABILITY_STATE_PENDING));
	/* Registration visibility survives an undeliverable reply. */
	request.result = 1;
	expect_ioctl_error(fd, DRM_IOCTL_CASTKMS_RENDERER_REGISTER_PROFILE, &request, EFAULT);
	state = query_capabilities(fd);
	CHECK(state.flags == DRM_CASTKMS_CAPABILITY_STATE_PENDING);
	request.result = (uintptr_t)&result;
	expect_ioctl_error(fd, DRM_IOCTL_CASTKMS_RENDERER_REGISTER_PROFILE, &request, EBUSY);
	return state.transition;
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
	struct drm_castkms_renderer_release_source release_source = {
		.completion_fd = -1,
	};
	drmModeConnector *connector;
	drmModeRes *resources;
	struct buffer buffer, capture_output, gpu_buffer, private_buffer;
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
	CHECK(drmSetClientCap(fd, DRM_CLIENT_CAP_ATOMIC, 1) == 0);
	check_plane_blending(fd);
	peer = open(argv[1], O_RDWR | O_CLOEXEC);
	CHECK(peer >= 0 && !drmIsMaster(peer));
	CHECK(drmSetClientCap(peer, DRM_CLIENT_CAP_ATOMIC, 1) == 0);
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
	check_transition_property(fd, request.crtc_id);
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
	expect_ioctl_error(files.renderer_fd,
			   DRM_IOCTL_CASTKMS_RENDERER_COMMIT_TAKEOVER,
			   &commit, EINVAL);
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
	connector = drmModeGetConnector(peer, connector_id);
	CHECK(connector && connector->count_modes > 0);
	gpu_buffer = create_buffer(peer, connector->modes[0].hdisplay,
				   connector->modes[0].vdisplay, 0x91);
	CHECK(drmModeSetCrtc(peer, request.crtc_id, gpu_buffer.fb, 0, 0,
			     &connector_id, 1, &connector->modes[0]) == 0);
	drmModeFreeConnector(connector);
	next_files = create_renderer(peer, &request);
	candidate = begin_takeover(next_files.renderer_fd, first.generation);
	submit_probe(next_files.renderer_fd, candidate.candidate_id,
		     DRM_CASTKMS_RENDERER_PROBE_PRIVATE);
	uint64_t transition = register_linear_profile(next_files.renderer_fd,
						      candidate.candidate_id,
						      gpu_buffer.dumb.width, gpu_buffer.dumb.height);
	struct drm_castkms_renderer_capabilities pending_caps =
		query_capabilities(next_files.renderer_fd);
	private_buffer = create_buffer(peer, gpu_buffer.dumb.width,
				       gpu_buffer.dumb.height, 0);
	int private_fd;

	CHECK(drmPrimeHandleToFD(peer, private_buffer.dumb.handle,
				 DRM_CLOEXEC | DRM_RDWR, &private_fd) == 0);
	struct drm_castkms_renderer_register_image private_image = {
		.image_id = 1, .buffers = (uintptr_t)&private_fd,
		.width = gpu_buffer.dumb.width, .height = gpu_buffer.dumb.height,
		.num_buffers = 1,
	};
	private_image.flags = 1;
	expect_ioctl_error(next_files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_REGISTER_IMAGE,
			   &private_image, EINVAL);
	private_image.flags = 0;
	private_image.buffers = 1;
	expect_ioctl_error(next_files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_REGISTER_IMAGE,
			   &private_image, EFAULT);
	private_image.buffers = (uintptr_t)&private_fd;
	private_image.width++;
	expect_ioctl_error(next_files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_REGISTER_IMAGE,
			   &private_image, EOPNOTSUPP);
	private_image.width -= 2;
	expect_ioctl_error(next_files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_REGISTER_IMAGE,
			   &private_image, EOPNOTSUPP);
	private_image.width++;
	CHECK(ioctl(next_files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_REGISTER_IMAGE,
		    &private_image) == 0);
	CHECK(close(private_fd) == 0);
	tag_transition(peer, request.crtc_id, transition, true);
	CHECK(query_capabilities(next_files.renderer_fd).flags == DRM_CASTKMS_CAPABILITY_STATE_PENDING);
	commit.candidate_id = candidate.candidate_id;
	expect_ioctl_error(next_files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_COMMIT_TAKEOVER,
			   &commit, EAGAIN);
	tag_transition(peer, request.crtc_id, transition, false);
	CHECK(query_capabilities(next_files.renderer_fd).flags ==
	      (DRM_CASTKMS_CAPABILITY_STATE_PENDING | DRM_CASTKMS_CAPABILITY_STATE_GATED));
	commit_takeover(next_files.renderer_fd, candidate.candidate_id);
	commit_takeover(next_files.renderer_fd, candidate.candidate_id);
	struct drm_castkms_renderer_capabilities active_caps =
		query_capabilities(next_files.renderer_fd);
	CHECK(active_caps.flags == 0);
	CHECK(active_caps.active_generation == pending_caps.pending_generation);
	CHECK(active_caps.validation_epoch == pending_caps.validation_epoch + 2);
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
	struct drm_castkms_renderer_dequeue_scene scene_request = {
		.capacity = DRM_CASTKMS_RENDERER_SCENE_MAX_BYTES,
		.image_id = 1,
	};
	struct drm_castkms_renderer_unregister_image remove_image = { .image_id = 1 };
	expect_ioctl_error(next_files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_DEQUEUE_SCENE,
			   &scene_request, EINVAL);
	scene_request.result = 1;
	before = open_files();
	for (unsigned int i = 0; i < 8; i++)
		expect_ioctl_error(next_files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_DEQUEUE_SCENE,
				   &scene_request, EFAULT);
	CHECK(open_files() == before);
	scene_request.capacity = 0;
	expect_ioctl_error(next_files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_DEQUEUE_SCENE,
			   &scene_request, ENOSPC);
	void *scene_bytes = malloc(DRM_CASTKMS_RENDERER_SCENE_MAX_BYTES);
	CHECK(scene_bytes);
	memset(scene_bytes, 0xa5, DRM_CASTKMS_RENDERER_SCENE_MAX_BYTES);
	scene_request.result = (uintptr_t)scene_bytes;
	scene_request.capacity = DRM_CASTKMS_RENDERER_SCENE_MAX_BYTES;
	CHECK(ioctl(next_files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_DEQUEUE_SCENE,
		    &scene_request) == 0);
	struct drm_castkms_renderer_scene *scene = scene_bytes;
	struct drm_castkms_renderer_layer *layer = (void *)(scene + 1);
	CHECK(scene->job_id && scene->content_serial);
	expect_ioctl_error(next_files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_UNREGISTER_IMAGE,
			   &remove_image, EBUSY);
	CHECK(scene->version == DRM_CASTKMS_RENDERER_SCENE_VERSION);
	CHECK(scene->bytes == sizeof(*scene) + sizeof(*layer));
	CHECK(scene->layer_count == 1 && scene->producer_fd == -1 && !scene->reserved);
	CHECK(layer->format == DRM_FORMAT_XRGB8888);
	CHECK(layer->color_encoding == DRM_CASTKMS_YUV_ENCODING_BT601);
	CHECK(layer->color_range == DRM_CASTKMS_YUV_RANGE_FULL);
	CHECK(layer->width == gpu_buffer.dumb.width && layer->height == gpu_buffer.dumb.height);
	CHECK(layer->plane_count == 1 && layer->planes[0].dma_buf_fd >= 0);
	for (unsigned int i = layer->plane_count; i < DRM_CASTKMS_RENDERER_MAX_PLANES; i++)
		CHECK(layer->planes[i].dma_buf_fd == -1 && !layer->planes[i].pitch &&
		      !layer->planes[i].offset && !layer->planes[i].reserved);
	CHECK(fcntl(layer->planes[0].dma_buf_fd, F_GETFD) == FD_CLOEXEC);
	uint64_t first_content_serial = scene->content_serial;
	/* A pending source reader must return through libdrm, not spin in drmIoctl. */
	CHECK(drmSetClientCap(peer, DRM_CLIENT_CAP_ATOMIC_PREPARATION, 1) == 0);
	struct drm_mode_prepare_replace prepare = {
		.crtc_ids = (uintptr_t)&request.crtc_id, .count_crtcs = 1,
	};
	int ticket = ioctl(peer, DRM_IOCTL_MODE_PREPARE_REPLACE, &prepare);
	struct drm_prepare_query ticket_state = {};
	drmModeAtomicReq *prepared_update = drmModeAtomicAlloc();
	CHECK(ticket >= 0 && prepared_update);
	CHECK(ioctl(ticket, DRM_IOCTL_PREPARE_QUERY, &ticket_state) == 0);
	CHECK(ticket_state.status == DRM_PREPARE_PENDING);
	property(peer, prepared_update, request.crtc_id, DRM_MODE_OBJECT_CRTC,
		 "PREPARE_FD", ticket);
	alarm(5);
	CHECK(drmModeAtomicCommit(peer, prepared_update, DRM_MODE_ATOMIC_NONBLOCK, NULL) < 0);
	CHECK(errno == EBUSY);
	alarm(0);
	CHECK(ioctl(ticket, DRM_IOCTL_PREPARE_QUERY, &ticket_state) == 0);
	CHECK(ticket_state.status == DRM_PREPARE_PENDING);
	expect_ioctl_error(next_files.renderer_fd,
			   DRM_IOCTL_CASTKMS_RENDERER_DEQUEUE_SCENE,
			   &scene_request, EBUSY);
	release_source.job_id = scene->job_id;
	release_source.kind = DRM_CASTKMS_RENDERER_RELEASE_NO_ACCESS;
	release_source.job_id++;
	expect_ioctl_error(next_files.renderer_fd,
			   DRM_IOCTL_CASTKMS_RENDERER_RELEASE_SOURCE,
			   &release_source, ENOENT);
	release_source.job_id--;
	release_source.completion_fd = 0;
	expect_ioctl_error(next_files.renderer_fd,
			   DRM_IOCTL_CASTKMS_RENDERER_RELEASE_SOURCE,
			   &release_source, EINVAL);
	release_source.completion_fd = -1;
	CHECK(ioctl(next_files.renderer_fd,
		    DRM_IOCTL_CASTKMS_RENDERER_RELEASE_SOURCE,
		    &release_source) == 0);
	CHECK(ioctl(next_files.renderer_fd,
		    DRM_IOCTL_CASTKMS_RENDERER_RELEASE_SOURCE,
		    &release_source) == 0);
	expect_ioctl_error(next_files.renderer_fd,
			   DRM_IOCTL_CASTKMS_RENDERER_DEQUEUE_SCENE,
			   &scene_request, EBUSY);
	CHECK(close(layer->planes[0].dma_buf_fd) == 0);
	CHECK(ioctl(ticket, DRM_IOCTL_PREPARE_QUERY, &ticket_state) == 0);
	CHECK(ticket_state.status == DRM_PREPARE_READY);
	CHECK(drmModeAtomicCommit(peer, prepared_update, 0, NULL) == 0);
	CHECK(ioctl(ticket, DRM_IOCTL_PREPARE_QUERY, &ticket_state) == 0);
	CHECK(ticket_state.status == DRM_PREPARE_CONSUMED);
	drmModeAtomicFree(prepared_update);
	CHECK(close(ticket) == 0);
	CHECK(drmSetClientCap(peer, DRM_CLIENT_CAP_ATOMIC_PREPARATION, 0) == 0);
	connector = drmModeGetConnector(peer, connector_id);
	CHECK(connector && connector->count_modes > 0);
	CHECK(drmModeSetCrtc(peer, request.crtc_id, gpu_buffer.fb, 0, 0,
			     &connector_id, 1, &connector->modes[0]) == 0);
	drmModeFreeConnector(connector);
	memset(scene_bytes, 0xa5, DRM_CASTKMS_RENDERER_SCENE_MAX_BYTES);
	uint32_t overlay = overlay_plane(peer, request.crtc_id);
	CHECK(drmModeSetPlane(peer, overlay, request.crtc_id, gpu_buffer.fb, 0,
		0, 0, gpu_buffer.dumb.width, gpu_buffer.dumb.height,
		0, 0, gpu_buffer.dumb.width << 16, gpu_buffer.dumb.height << 16) == 0);
	CHECK(ioctl(next_files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_DEQUEUE_SCENE,
		    &scene_request) == 0);
	CHECK(scene->version == DRM_CASTKMS_RENDERER_SCENE_VERSION);
	CHECK(scene->bytes == sizeof(*scene) + 2 * sizeof(*layer));
	CHECK(scene->layer_count == 2 && scene->producer_fd == -1);
	CHECK(scene->output_color_count == 0 && scene->reserved == 0);
	CHECK(scene->content_serial != first_content_serial);
	CHECK(layer->bytes == sizeof(*layer) && layer->color_count == 0);
	CHECK(layer->kind == DRM_CASTKMS_RENDERER_LAYER_PRIMARY);
	CHECK(layer->format == DRM_FORMAT_XRGB8888 && layer->plane_count == 1);
	CHECK(layer->width == gpu_buffer.dumb.width && layer->height == gpu_buffer.dumb.height);
	CHECK(fcntl(layer->planes[0].dma_buf_fd, F_GETFD) == FD_CLOEXEC);
	release_source.job_id = scene->job_id;
	CHECK(ioctl(next_files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_RELEASE_SOURCE,
		    &release_source) == 0);
	CHECK(close(layer->planes[0].dma_buf_fd) == 0);
	CHECK(layer[1].kind == DRM_CASTKMS_RENDERER_LAYER_OVERLAY);
	CHECK(close(layer[1].planes[0].dma_buf_fd) == 0);
	uint64_t retry_serial = scene->content_serial;
	uint64_t previous_job = scene->job_id;
	CHECK(ioctl(next_files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_DEQUEUE_SCENE,
		    &scene_request) == 0);
	CHECK(scene->content_serial == retry_serial && scene->job_id > previous_job);
	CHECK(scene->layer_count == 2 && scene->producer_fd == -1);
	release_source.job_id = scene->job_id;
	CHECK(ioctl(next_files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_RELEASE_SOURCE,
		    &release_source) == 0);
	CHECK(close(layer->planes[0].dma_buf_fd) == 0);
	CHECK(close(layer[1].planes[0].dma_buf_fd) == 0);
	free(scene_bytes);
	CHECK(ioctl(next_files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_UNREGISTER_IMAGE,
		    &remove_image) == 0);
	expect_ioctl_error(next_files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_UNREGISTER_IMAGE,
			   &remove_image, ENOENT);
	/* A fresh endpoint requests fixed HOST policy while the GPU endpoint remains alive. */
	struct drm_castkms_renderer_files host_files = create_renderer(peer, &request);
	struct drm_castkms_renderer_takeover host_candidate =
		begin_takeover(host_files.renderer_fd, next.generation);
	CHECK(host_candidate.profile == DRM_CASTKMS_EXECUTION_GPU_V1);
	struct drm_castkms_capability_profile host_profile = {
		.version = DRM_CASTKMS_CAPABILITY_VERSION, .kind = DRM_CASTKMS_CAPABILITY_KIND_HOST,
	};
	struct drm_castkms_renderer_profile_result host_result;
	struct drm_castkms_renderer_register_profile host_request = {
		.candidate_id = host_candidate.candidate_id, .profile = (uintptr_t)&host_profile,
		.result = (uintptr_t)&host_result, .profile_size = sizeof(host_profile),
	};
	CHECK(ioctl(host_files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_REGISTER_PROFILE,
		    &host_request) == 0);
	tag_transition(peer, request.crtc_id, host_result.transition, false);
	commit_takeover(host_files.renderer_fd, host_candidate.candidate_id);
	commit_takeover(host_files.renderer_fd, host_candidate.candidate_id);
	CHECK(query_renderer(host_files.renderer_fd).generation == next.generation + 1);
	CHECK(query_capabilities(host_files.renderer_fd).active_generation ==
	      host_result.capability_generation);
	CHECK(close(host_files.renderer_fd) == 0);
	CHECK(close(host_files.revoke_fd) == 0);
	CHECK(close(next_files.renderer_fd) == 0);
	CHECK(close(next_files.revoke_fd) == 0);
	CHECK(close(files.revoke_fd) == 0);
	destroy_buffer(fd, &buffer);
	destroy_buffer(fd, &capture_output);
	destroy_buffer(peer, &gpu_buffer);
	destroy_buffer(peer, &private_buffer);
	CHECK(close(peer) == 0);
	CHECK(close(fd) == 0);
	puts("PASS: renderer capability publication, query and revocation");
	return 0;
}
