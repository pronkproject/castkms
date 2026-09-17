// SPDX-License-Identifier: GPL-2.0-only

/* Named stream admission and cleanup, without queueing or exporting pixels. */
#include "fixture.h"

#include <fcntl.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#include "../../../../include/uapi/drm/drm_capture.h"

_Static_assert(sizeof(struct drm_capture_create_stream) == 32, "stream creation ABI");
_Static_assert(sizeof(struct drm_capture_destroy_stream) == 16, "stream destruction ABI");

static int create_stream(int fd, uint64_t id, uint64_t offer, uint32_t capacity)
{
	struct drm_capture_create_stream input = {
		.id = id, .offer = offer, .capacity = capacity,
	};

	return drmIoctl(fd, DRM_IOCTL_CAPTURE_CREATE_STREAM, &input);
}

static int destroy_stream(int fd, uint64_t id)
{
	struct drm_capture_destroy_stream input = { .id = id };

	return drmIoctl(fd, DRM_IOCTL_CAPTURE_DESTROY_STREAM, &input);
}

static void malformed_requests(int fd, int control, uint64_t offer)
{
	struct drm_capture_create_stream input = { .id = 1, .offer = offer, .capacity = 1 };
	struct drm_capture_destroy_stream close = { .id = 1, .reserved = 1 };
	long page_size = sysconf(_SC_PAGESIZE);
	void *mapping, *partial;

	CHECK(page_size > 0);
	CHECK(create_stream(control, 1, offer, 1) == -1 && errno == ENOTTY);
	CHECK(destroy_stream(control, 1) == -1 && errno == ENOTTY);
	CHECK(create_stream(fd, 0, offer, 1) == -1 && errno == EINVAL);
	CHECK(create_stream(fd, 1, 0, 1) == -1 && errno == EINVAL);
	CHECK(create_stream(fd, 1, offer, 0) == -1 && errno == EINVAL);
	CHECK(create_stream(fd, 1, offer, 9) == -1 && errno == E2BIG);
	CHECK(create_stream(fd, 1, offer + 1, 1) == -1 && errno == ESTALE);
	CHECK(destroy_stream(fd, 0) == -1 && errno == EINVAL);
	CHECK(destroy_stream(fd, 1) == -1 && errno == ENOENT);
	input.flags = 1;
	CHECK(drmIoctl(fd, DRM_IOCTL_CAPTURE_CREATE_STREAM, &input) == -1 && errno == EINVAL);
	input.flags = 0;
	input.reserved = 1;
	CHECK(drmIoctl(fd, DRM_IOCTL_CAPTURE_CREATE_STREAM, &input) == -1 && errno == EINVAL);
	input.reserved = 0;
	CHECK(drmIoctl(fd, DRM_IOCTL_CAPTURE_DESTROY_STREAM, &close) == -1 && errno == EINVAL);
	CHECK(drmIoctl(fd, DRM_IOCTL_CAPTURE_CREATE_STREAM ^ (1U << _IOC_SIZESHIFT),
		       &input) == -1 && errno == ENOTTY);
	for (unsigned int i = 0; i < 128; i++) {
		CHECK(drmIoctl(fd, DRM_IOCTL_CAPTURE_CREATE_STREAM, (void *)1) == -1);
		CHECK(errno == EFAULT);
		CHECK(drmIoctl(fd, DRM_IOCTL_CAPTURE_DESTROY_STREAM, (void *)1) == -1);
		CHECK(errno == EFAULT);
	}
	mapping = mmap(NULL, page_size * 2, PROT_READ | PROT_WRITE,
		       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	CHECK(mapping != MAP_FAILED);
	partial = (char *)mapping + page_size - sizeof(input.id);
	memcpy(partial, &input.id, sizeof(input.id));
	CHECK(mprotect((char *)mapping + page_size, page_size, PROT_NONE) == 0);
	CHECK(drmIoctl(fd, DRM_IOCTL_CAPTURE_CREATE_STREAM, partial) == -1 && errno == EFAULT);
	/* Read-only request memory suffices: admission has no later copyout. */
	memcpy(mapping, &input, sizeof(input));
	CHECK(mprotect(mapping, page_size, PROT_READ) == 0);
	CHECK(drmIoctl(fd, DRM_IOCTL_CAPTURE_CREATE_STREAM, mapping) == 0);
	CHECK(drmIoctl(fd, DRM_IOCTL_CAPTURE_DESTROY_STREAM, partial) == -1 && errno == EFAULT);
	CHECK(munmap(mapping, page_size * 2) == 0);
	/* A malformed close must not have removed the admitted stream. */
	CHECK(drmIoctl(fd, DRM_IOCTL_CAPTURE_DESTROY_STREAM, &close) == -1 && errno == EINVAL);
	CHECK(create_stream(fd, 1, offer, 1) == -1 && errno == ESTALE);
}

int main(int argc, char **argv)
{
	struct monitor_control monitor;
	struct drm_mode_create_capture_grant grant = {};
	struct drm_capture_grant_files files;
	struct drm_capture_describe first, next;
	struct buffer buffer;
	drmModeRes *resources;
	drmModeConnector *connector;
	drmModeModeInfo mode = {};
	drmVersion *version;
	int master, duplicate;

	CHECK(argc == 2);
	master = open(argv[1], O_RDWR | O_CLOEXEC);
	CHECK(master >= 0 && drmIsMaster(master));
	version = drmGetVersion(master);
	CHECK(version && !strcmp(version->name, "castkms"));
	drmFreeVersion(version);
	resources = drmModeGetResources(master);
	CHECK(resources && resources->count_crtcs > 0 && resources->count_connectors > 0);
	grant.crtc_id = resources->crtcs[0];
	grant.connector_id = resources->connectors[0];
	monitor = attach_fallback_monitor(master, grant.connector_id);
	grant.files = (uintptr_t)&files;
	drmModeFreeResources(resources);
	connector = drmModeGetConnector(master, grant.connector_id);
	CHECK(connector);
	/* Isolate stream-count limits from the independent private-image byte budget. */
	for (int i = 0; i < connector->count_modes; i++) {
		if (connector->modes[i].hdisplay == 640 && connector->modes[i].vdisplay == 480) {
			mode = connector->modes[i];
			break;
		}
	}
	drmModeFreeConnector(connector);
	CHECK(mode.clock);
	buffer = create_buffer(master, mode.hdisplay, mode.vdisplay, 0x49);
	CHECK(drmModeSetCrtc(master, grant.crtc_id, buffer.fb, 0, 0,
			     &grant.connector_id, 1, &mode) == 0);
	CHECK(drmIoctl(master, DRM_IOCTL_MODE_CREATE_CAPTURE_GRANT, &grant) == 0);
	CHECK(drmIoctl(files.capture_fd, DRM_IOCTL_CAPTURE_DESCRIBE, &first) == 0);
	malformed_requests(files.capture_fd, files.control_fd, first.id);
	duplicate = fcntl(files.capture_fd, F_DUPFD_CLOEXEC, 0);
	CHECK(duplicate >= 0);
	CHECK(create_stream(duplicate, 1, first.id, 1) == -1 && errno == ESTALE);
	for (uint64_t id = 2; id <= 16; id++)
		CHECK(create_stream(files.capture_fd, id, first.id, 1) == 0);
	CHECK(create_stream(files.capture_fd, 17, first.id, 1) == -1 && errno == EBUSY);
	CHECK(destroy_stream(duplicate, 1) == 0);
	CHECK(destroy_stream(files.capture_fd, 1) == -1 && errno == ENOENT);
	CHECK(create_stream(files.capture_fd, 1, first.id, 1) == -1 && errno == ESTALE);
	CHECK(create_stream(files.capture_fd, 17, first.id, 1) == 0);
	for (uint64_t id = 2; id <= 16; id++)
		CHECK(destroy_stream(files.capture_fd, id) == 0);
	mode.clock++;
	CHECK(drmModeSetCrtc(master, grant.crtc_id, buffer.fb, 0, 0,
			     &grant.connector_id, 1, &mode) == 0);
	CHECK(drmIoctl(files.capture_fd, DRM_IOCTL_CAPTURE_DESCRIBE, &next) == 0);
	CHECK(next.id != first.id);
	CHECK(create_stream(files.capture_fd, 18, first.id, 1) == -1 && errno == ESTALE);
	CHECK(create_stream(files.capture_fd, 18, next.id, 1) == 0);
	CHECK(destroy_stream(files.capture_fd, 17) == 0);
	CHECK(create_stream(files.capture_fd, UINT64_MAX, next.id, 1) == 0);
	CHECK(create_stream(files.capture_fd, 19, next.id, 1) == -1 && errno == EOVERFLOW);
	CHECK(close(files.control_fd) == 0);
	CHECK(create_stream(files.capture_fd, 19, next.id, 1) == -1 && errno == EKEYREVOKED);
	CHECK(destroy_stream(files.capture_fd, 18) == 0);
	CHECK(destroy_stream(files.capture_fd, UINT64_MAX) == 0);
	CHECK(destroy_stream(files.capture_fd, UINT64_MAX) == -1 && errno == ENOENT);
	CHECK(close(duplicate) == 0);
	CHECK(close(files.capture_fd) == 0);
	/* A fresh client starts a fresh namespace after the old client's resources leave. */
	CHECK(drmIoctl(master, DRM_IOCTL_MODE_CREATE_CAPTURE_GRANT, &grant) == 0);
	CHECK(drmIoctl(files.capture_fd, DRM_IOCTL_CAPTURE_DESCRIBE, &next) == 0);
	for (uint64_t id = 1; id <= 16; id++)
		CHECK(create_stream(files.capture_fd, id, next.id, 1) == 0);
	CHECK(close(files.capture_fd) == 0);
	CHECK(close(files.control_fd) == 0);
	CHECK(drmIoctl(master, DRM_IOCTL_MODE_CREATE_CAPTURE_GRANT, &grant) == 0);
	CHECK(drmIoctl(files.capture_fd, DRM_IOCTL_CAPTURE_DESCRIBE, &next) == 0);
	for (uint64_t id = 1; id <= 16; id++)
		CHECK(create_stream(files.capture_fd, id, next.id, 1) == 0);
	CHECK(close(files.capture_fd) == 0);
	CHECK(close(files.control_fd) == 0);
	CHECK(drmModeSetCrtc(master, grant.crtc_id, 0, 0, 0, NULL, 0, NULL) == 0);
	destroy_buffer(master, &buffer);
	close_monitor(&monitor);
	CHECK(close(master) == 0);
	puts("PASS: capture stream admission, shared names, rollback and revoked cleanup");
	return 0;
}
