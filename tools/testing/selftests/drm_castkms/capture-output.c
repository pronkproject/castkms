// SPDX-License-Identifier: GPL-2.0-only

/* Public output admission, fault-safe acknowledgment and cancellation. */
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

#include "../../../../include/uapi/drm/drm_capture.h"

_Static_assert(sizeof(struct drm_capture_queue_output) == 40, "queue ABI");
_Static_assert(sizeof(struct drm_capture_result) == 24, "result ABI");
_Static_assert(sizeof(struct drm_capture_dequeue) == 24, "dequeue ABI");
_Static_assert(sizeof(struct drm_capture_cancel) == 24, "cancel ABI");

static int queue(int fd, uint64_t use_id, uint64_t destination, int reuse)
{
	struct drm_capture_queue_output input = {
		.stream = 1, .use_id = use_id, .destination = destination, .reuse_fd = reuse,
	};

	return ioctl(fd, DRM_IOCTL_CAPTURE_QUEUE_OUTPUT, &input);
}

static int cancel(int fd, uint64_t use_id)
{
	struct drm_capture_cancel input = { .stream = 1, .use_id = use_id };

	return ioctl(fd, DRM_IOCTL_CAPTURE_CANCEL, &input);
}

static int dequeue(int fd, void *result)
{
	struct drm_capture_dequeue input = { .stream = 1, .result = (uintptr_t)result };

	return ioctl(fd, DRM_IOCTL_CAPTURE_DEQUEUE, &input);
}

static void wait_result(int fd)
{
	struct pollfd event = { .fd = fd, .events = POLLIN };

	CHECK(poll(&event, 1, 5000) == 1);
	CHECK(event.revents & POLLIN);
	CHECK(!(event.revents & (POLLERR | POLLNVAL)));
}

static void register_output(int client, uint64_t id, int dma, const struct buffer *buffer)
{
	struct drm_capture_register_destination input = {
		.id = id, .width = 640, .height = 480, .format = DRM_FORMAT_XRGB8888,
		.num_planes = 1, .modifier = DRM_FORMAT_MOD_LINEAR,
		.fds = { dma }, .strides = { buffer->dumb.pitch },
	};

	CHECK(ioctl(client, DRM_IOCTL_CAPTURE_REGISTER_DESTINATION, &input) == 0);
}

static void check_pixels(int dma, const struct buffer *buffer, unsigned char value,
			 bool composed)
{
	struct dma_buf_sync sync = { .flags = DMA_BUF_SYNC_START | DMA_BUF_SYNC_READ };
	unsigned char *pixels = mmap(NULL, buffer->dumb.size, PROT_READ, MAP_SHARED, dma, 0);

	CHECK(pixels != MAP_FAILED);
	CHECK(ioctl(dma, DMA_BUF_IOCTL_SYNC, &sync) == 0);
	for (unsigned int y = 0; y < 480; y++) {
		for (unsigned int x = 0; x < 640 * 4; x++)
			CHECK(pixels[y * buffer->dumb.pitch + x] ==
			      (composed && x % 4 == 3 ? 0xff : value));
	}
	sync.flags = DMA_BUF_SYNC_END | DMA_BUF_SYNC_READ;
	CHECK(ioctl(dma, DMA_BUF_IOCTL_SYNC, &sync) == 0);
	CHECK(munmap(pixels, buffer->dumb.size) == 0);
}

static void malformed_requests(int client, int control, int dma)
{
	struct drm_capture_queue_output input = {
		.stream = 1, .use_id = 1, .destination = 1, .reuse_fd = -1,
	};
	struct drm_capture_cancel stop = { .stream = 1, .use_id = 1, .reserved = 1 };
	struct drm_capture_dequeue take = { .stream = 1, .result = 1, .reserved = 1 };

	CHECK(queue(control, 1, 1, -1) == -1 && errno == ENOTTY);
	CHECK(cancel(control, 1) == -1 && errno == ENOTTY);
	CHECK(dequeue(control, (void *)1) == -1 && errno == ENOTTY);
	CHECK(queue(client, 0, 1, -1) == -1 && errno == EINVAL);
	CHECK(queue(client, 1, 0, -1) == -1 && errno == EINVAL);
	CHECK(queue(client, 1, 9, -1) == -1 && errno == ENOENT);
	CHECK(queue(client, 1, 1, -2) == -1 && errno == EINVAL);
	CHECK(queue(client, 1, 1, dma) == -1 && errno == EINVAL);
	input.flags = 1;
	CHECK(ioctl(client, DRM_IOCTL_CAPTURE_QUEUE_OUTPUT, &input) == -1 && errno == EINVAL);
	input.flags = 0;
	input.reserved = 1;
	CHECK(ioctl(client, DRM_IOCTL_CAPTURE_QUEUE_OUTPUT, &input) == -1 && errno == EINVAL);
	CHECK(ioctl(client, DRM_IOCTL_CAPTURE_DEQUEUE, &take) == -1 && errno == EINVAL);
	CHECK(ioctl(client, DRM_IOCTL_CAPTURE_CANCEL, &stop) == -1 && errno == EINVAL);
	CHECK(cancel(client, 0) == -1 && errno == EINVAL);
	CHECK(cancel(client, 1) == -1 && errno == ENOENT);
	CHECK(dequeue(client, (void *)1) == -1 && errno == EAGAIN);
	CHECK(ioctl(client, DRM_IOCTL_CAPTURE_QUEUE_OUTPUT, (void *)1) == -1 && errno == EFAULT);
	CHECK(ioctl(client, DRM_IOCTL_CAPTURE_DEQUEUE, (void *)1) == -1 && errno == EFAULT);
	CHECK(ioctl(client, DRM_IOCTL_CAPTURE_CANCEL, (void *)1) == -1 && errno == EFAULT);
	CHECK(ioctl(client, DRM_IOCTL_CAPTURE_QUEUE_OUTPUT ^ (1U << _IOC_SIZESHIFT),
		    &input) == -1 && errno == ENOTTY);
}

static void submit_read_only(int client, int reuse)
{
	struct drm_capture_queue_output input = {
		.stream = 1, .use_id = 1, .destination = 1, .reuse_fd = reuse,
	};
	long page_size = sysconf(_SC_PAGESIZE);
	void *mapping;

	CHECK(page_size > 0);
	mapping = mmap(NULL, page_size, PROT_READ | PROT_WRITE,
		       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	CHECK(mapping != MAP_FAILED);
	memcpy(mapping, &input, sizeof(input));
	CHECK(mprotect(mapping, page_size, PROT_READ) == 0);
	CHECK(ioctl(client, DRM_IOCTL_CAPTURE_QUEUE_OUTPUT, mapping) == 0);
	CHECK(munmap(mapping, page_size) == 0);
}

static void failed_publication(int client)
{
	long page_size = sysconf(_SC_PAGESIZE);
	void *mapping, *partial;

	CHECK(page_size > 0);
	mapping = mmap(NULL, page_size * 2, PROT_READ | PROT_WRITE,
		       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	CHECK(mapping != MAP_FAILED);
	partial = (char *)mapping + page_size - sizeof(uint64_t);
	CHECK(mprotect((char *)mapping + page_size, page_size, PROT_NONE) == 0);
	for (unsigned int i = 0; i < 8; i++) {
		CHECK(dequeue(client, (void *)1) == -1 && errno == EFAULT);
		CHECK(dequeue(client, partial) == -1 && errno == EFAULT);
		CHECK(queue(client, 2, 2, -1) == -1 && errno == EAGAIN);
		wait_result(client);
	}
	CHECK(munmap(mapping, page_size * 2) == 0);
}

int main(int argc, char **argv)
{
	struct drm_mode_create_capture_grant grant = {};
	struct drm_capture_grant_files files;
	struct drm_capture_describe description;
	struct drm_capture_create_stream stream = { .id = 1, .capacity = 1 };
	struct drm_capture_destroy_stream close_stream = { .id = 1 };
	struct drm_capture_unregister_destination remove = { .id = 1 };
	struct dma_buf_export_sync_file reuse = { .flags = DMA_BUF_SYNC_RW };
	struct drm_capture_result result;
	struct buffer source, changed, output, replacement;
	drmModeRes *resources;
	drmModePlaneRes *planes;
	drmModeConnector *connector;
	drmModeModeInfo mode = {};
	drmVersion *version;
	struct pollfd event;
	uint32_t plane;
	int master, output_fd, replacement_fd, source_fd, duplicate, cancelled;

	CHECK(argc == 2);
	master = open(argv[1], O_RDWR | O_CLOEXEC);
	CHECK(master >= 0 && drmIsMaster(master));
	CHECK(drmSetClientCap(master, DRM_CLIENT_CAP_ATOMIC, 1) == 0);
	version = drmGetVersion(master);
	CHECK(version && !strcmp(version->name, "castkms"));
	drmFreeVersion(version);
	resources = drmModeGetResources(master);
	CHECK(resources && resources->count_crtcs == 1 && resources->count_connectors == 1);
	grant.crtc_id = resources->crtcs[0];
	grant.connector_id = resources->connectors[0];
	grant.files = (uintptr_t)&files;
	drmModeFreeResources(resources);
	planes = drmModeGetPlaneResources(master);
	CHECK(planes && planes->count_planes == 1);
	plane = planes->planes[0];
	drmModeFreePlaneResources(planes);
	connector = drmModeGetConnector(master, grant.connector_id);
	CHECK(connector);
	for (int i = 0; i < connector->count_modes; i++) {
		if (connector->modes[i].hdisplay == 640 && connector->modes[i].vdisplay == 480) {
			mode = connector->modes[i];
			break;
		}
	}
	drmModeFreeConnector(connector);
	CHECK(mode.clock);
	source = create_buffer(master, 640, 480, 0x49);
	changed = create_buffer(master, 640, 480, 0x68);
	output = create_buffer(master, 640, 480, 0x55);
	replacement = create_buffer(master, 640, 480, 0x33);
	CHECK(drmPrimeHandleToFD(master, source.dumb.handle, DRM_CLOEXEC | DRM_RDWR,
				 &source_fd) == 0);
	CHECK(drmPrimeHandleToFD(master, output.dumb.handle, DRM_CLOEXEC | DRM_RDWR,
				 &output_fd) == 0);
	CHECK(drmPrimeHandleToFD(master, replacement.dumb.handle, DRM_CLOEXEC | DRM_RDWR,
				 &replacement_fd) == 0);
	CHECK(drmModeSetCrtc(master, grant.crtc_id, source.fb, 0, 0,
			     &grant.connector_id, 1, &mode) == 0);
	CHECK(ioctl(master, DRM_IOCTL_MODE_CREATE_CAPTURE_GRANT, &grant) == 0);
	CHECK(ioctl(files.capture_fd, DRM_IOCTL_CAPTURE_DESCRIBE, &description) == 0);
	stream.offer = description.id;
	CHECK(ioctl(files.capture_fd, DRM_IOCTL_CAPTURE_CREATE_STREAM, &stream) == 0);
	register_output(files.capture_fd, 1, output_fd, &output);
	register_output(files.capture_fd, 2, replacement_fd, &replacement);
	register_output(files.capture_fd, 3, source_fd, &source);
	CHECK(queue(files.capture_fd, 1, 3, -1) == -1 && errno == EINVAL);
	malformed_requests(files.capture_fd, files.control_fd, output_fd);
	CHECK(ioctl(output_fd, DMA_BUF_IOCTL_EXPORT_SYNC_FILE, &reuse) == 0);
	submit_read_only(files.capture_fd, reuse.fd);
	CHECK(close(reuse.fd) == 0);
	CHECK(ioctl(files.capture_fd, DRM_IOCTL_CAPTURE_UNREGISTER_DESTINATION, &remove) == 0);
	wait_result(files.capture_fd);
	CHECK(cancel(files.capture_fd, 1) == -1 && errno == EALREADY);
	failed_publication(files.capture_fd);
	duplicate = fcntl(files.capture_fd, F_DUPFD_CLOEXEC, 0);
	CHECK(duplicate >= 0);
	memset(&result, 0xa5, sizeof(result));
	CHECK(dequeue(duplicate, &result) == 0);
	CHECK(result.use_id == 1 && result.status == 0);
	CHECK(result.completed_at_ns > 0 && !result.reserved);
	CHECK(dequeue(files.capture_fd, &result) == -1 && errno == EAGAIN);
	CHECK(cancel(files.capture_fd, 1) == -1 && errno == ENOENT);
	CHECK(queue(files.capture_fd, 1, 2, -1) == -1 && errno == ESTALE);
	check_pixels(output_fd, &output, 0x49, true);
	check_pixels(replacement_fd, &replacement, 0x33, false);
	CHECK(queue(files.capture_fd, 2, 2, -1) == 0);
	cancelled = cancel(duplicate, 2);
	CHECK(cancelled == 0 || (cancelled == -1 && errno == EALREADY));
	wait_result(files.capture_fd);
	CHECK(dequeue(files.capture_fd, &result) == 0);
	CHECK(result.use_id == 2 && !result.reserved);
	CHECK(result.status == (cancelled == 0 ? -ECANCELED : 0));
	CHECK(cancelled == 0 ? result.completed_at_ns == 0 : result.completed_at_ns > 0);
	flip(master, plane, changed.fb);
	CHECK(queue(files.capture_fd, 3, 2, -1) == 0);
	wait_result(files.capture_fd);
	CHECK(close(files.control_fd) == 0);
	event = (struct pollfd){ .fd = files.capture_fd, .events = POLLIN };
	CHECK(poll(&event, 1, 5000) == 1);
	CHECK((event.revents & (POLLIN | POLLHUP)) == (POLLIN | POLLHUP));
	CHECK(queue(files.capture_fd, 4, 2, -1) == -1 && errno == EKEYREVOKED);
	CHECK(cancel(files.capture_fd, 3) == -1 && errno == EALREADY);
	CHECK(dequeue(duplicate, &result) == 0 && result.use_id == 3 && result.status == 0);
	check_pixels(replacement_fd, &replacement, 0x68, true);
	CHECK(ioctl(files.capture_fd, DRM_IOCTL_CAPTURE_DESTROY_STREAM, &close_stream) == 0);
	CHECK(cancel(files.capture_fd, 3) == -1 && errno == ENOENT);
	CHECK(close(duplicate) == 0);
	CHECK(close(files.capture_fd) == 0);
	CHECK(close(source_fd) == 0);
	CHECK(close(output_fd) == 0);
	CHECK(close(replacement_fd) == 0);
	CHECK(drmModeSetCrtc(master, grant.crtc_id, 0, 0, 0, NULL, 0, NULL) == 0);
	destroy_buffer(master, &source);
	destroy_buffer(master, &changed);
	destroy_buffer(master, &output);
	destroy_buffer(master, &replacement);
	CHECK(close(master) == 0);
	puts("PASS: capture output, retained faults, cancellation and revoked dequeue");
	return 0;
}
