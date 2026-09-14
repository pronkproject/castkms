// SPDX-License-Identifier: GPL-2.0-only

/* Input-only destination registration, descriptor lifetimes and revoked cleanup. */
#include "fixture.h"

#include <fcntl.h>
#include <drm_fourcc.h>
#include <string.h>
#include <sys/mman.h>
#include <unistd.h>

#include "../../../../include/uapi/drm/drm_capture.h"

_Static_assert(sizeof(struct drm_capture_register_destination) == 112, "destination ABI");
_Static_assert(sizeof(struct drm_capture_unregister_destination) == 16, "removal ABI");

static int remove_destination(int fd, uint64_t id)
{
	struct drm_capture_unregister_destination input = { .id = id };

	return drmIoctl(fd, DRM_IOCTL_CAPTURE_UNREGISTER_DESTINATION, &input);
}

static int register_destination(int fd, struct drm_capture_register_destination *input, uint64_t id)
{
	input->id = id;
	return drmIoctl(fd, DRM_IOCTL_CAPTURE_REGISTER_DESTINATION, input);
}

static void malformed_requests(int fd, int control, int read_only,
			       struct drm_capture_register_destination *valid)
{
	struct drm_capture_register_destination input = *valid;
	struct drm_capture_unregister_destination remove = { .id = 1, .reserved = 1 };
	long page_size = sysconf(_SC_PAGESIZE);
	void *mapping, *partial;

	CHECK(register_destination(control, &input, 1) == -1 && errno == ENOTTY);
	CHECK(remove_destination(control, 1) == -1 && errno == ENOTTY);
	CHECK(register_destination(fd, &input, 0) == -1 && errno == EINVAL);
	CHECK(remove_destination(fd, 0) == -1 && errno == EINVAL);
	CHECK(remove_destination(fd, 1) == -1 && errno == ENOENT);
	input.flags = 1;
	CHECK(register_destination(fd, &input, 1) == -1 && errno == EINVAL);
	input = *valid;
	input.reserved[2] = 1;
	CHECK(register_destination(fd, &input, 1) == -1 && errno == EINVAL);
	input = *valid;
	input.fds[3] = 1;
	CHECK(register_destination(fd, &input, 1) == -1 && errno == EINVAL);
	input = *valid;
	input.num_planes = 5;
	CHECK(register_destination(fd, &input, 1) == -1 && errno == EINVAL);
	input = *valid;
	input.fds[0] = read_only;
	CHECK(register_destination(fd, &input, 1) == -1 && errno == EACCES);
	input = *valid;
	input.offsets[0] = 4;
	CHECK(register_destination(fd, &input, 1) == -1 && errno == EINVAL);
	input = *valid;
	input.num_planes = 2;
	input.fds[1] = -1;
	input.strides[1] = input.strides[0];
	for (unsigned int i = 0; i < 128; i++) {
		CHECK(register_destination(fd, &input, 1) == -1 && errno == EBADF);
		CHECK(drmIoctl(fd, DRM_IOCTL_CAPTURE_REGISTER_DESTINATION, (void *)1) == -1);
		CHECK(errno == EFAULT);
		CHECK(drmIoctl(fd, DRM_IOCTL_CAPTURE_UNREGISTER_DESTINATION, (void *)1) == -1);
		CHECK(errno == EFAULT);
	}
	input.fds[1] = input.fds[0];
	CHECK(register_destination(fd, &input, 1) == -1 && errno == EOPNOTSUPP);
	input = *valid;
	CHECK(drmIoctl(fd, DRM_IOCTL_CAPTURE_REGISTER_DESTINATION ^ (1U << _IOC_SIZESHIFT),
		       &input) == -1 && errno == ENOTTY);
	CHECK(page_size > 0);
	mapping = mmap(NULL, page_size * 2, PROT_READ | PROT_WRITE,
		       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	CHECK(mapping != MAP_FAILED);
	partial = (char *)mapping + page_size - sizeof(input.id);
	memcpy(partial, &input.id, sizeof(input.id));
	CHECK(mprotect((char *)mapping + page_size, page_size, PROT_NONE) == 0);
	CHECK(drmIoctl(fd, DRM_IOCTL_CAPTURE_REGISTER_DESTINATION, partial) == -1 && errno == EFAULT);
	memcpy(mapping, &input, sizeof(input));
	CHECK(mprotect(mapping, page_size, PROT_READ) == 0);
	CHECK(drmIoctl(fd, DRM_IOCTL_CAPTURE_REGISTER_DESTINATION, mapping) == 0);
	CHECK(drmIoctl(fd, DRM_IOCTL_CAPTURE_UNREGISTER_DESTINATION, partial) == -1 && errno == EFAULT);
	CHECK(munmap(mapping, page_size * 2) == 0);
	CHECK(drmIoctl(fd, DRM_IOCTL_CAPTURE_UNREGISTER_DESTINATION, &remove) == -1 && errno == EINVAL);
	CHECK(register_destination(fd, &input, 1) == -1 && errno == ESTALE);
}

int main(int argc, char **argv)
{
	struct drm_mode_create_capture_grant grant = {};
	struct drm_capture_grant_files files;
	struct drm_capture_describe description;
	struct drm_capture_create_stream stream = { .id = 1, .capacity = 1 };
	struct drm_capture_destroy_stream close_stream = { .id = 1 };
	struct drm_capture_register_destination destination = {
		.id = 1, .width = 640, .height = 480, .format = DRM_FORMAT_XRGB8888,
		.num_planes = 1, .modifier = DRM_FORMAT_MOD_LINEAR,
	};
	struct buffer scanout, output, read_only;
	drmModeRes *resources;
	drmModeConnector *connector;
	drmModeModeInfo mode = {};
	drmVersion *version;
	int master, duplicate, readonly_fd, output_fd;

	CHECK(argc == 2);
	master = open(argv[1], O_RDWR | O_CLOEXEC);
	CHECK(master >= 0 && drmIsMaster(master));
	version = drmGetVersion(master);
	CHECK(version && !strcmp(version->name, "castkms"));
	drmFreeVersion(version);
	resources = drmModeGetResources(master);
	CHECK(resources && resources->count_crtcs == 1 && resources->count_connectors == 1);
	grant.crtc_id = resources->crtcs[0];
	grant.connector_id = resources->connectors[0];
	grant.files = (uintptr_t)&files;
	drmModeFreeResources(resources);
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
	scanout = create_buffer(master, 640, 480, 0x49);
	output = create_buffer(master, 640, 480, 0x55);
	read_only = create_buffer(master, 640, 480, 0x33);
	CHECK(drmPrimeHandleToFD(master, output.dumb.handle, DRM_CLOEXEC | DRM_RDWR, &output_fd) == 0);
	CHECK(drmPrimeHandleToFD(master, read_only.dumb.handle, DRM_CLOEXEC, &readonly_fd) == 0);
	destination.fds[0] = output_fd;
	destination.strides[0] = output.dumb.pitch;
	CHECK(drmModeSetCrtc(master, grant.crtc_id, scanout.fb, 0, 0,
			     &grant.connector_id, 1, &mode) == 0);
	CHECK(drmIoctl(master, DRM_IOCTL_MODE_CREATE_CAPTURE_GRANT, &grant) == 0);
	CHECK(drmIoctl(files.capture_fd, DRM_IOCTL_CAPTURE_DESCRIBE, &description) == 0);
	stream.offer = description.id;
	CHECK(drmIoctl(files.capture_fd, DRM_IOCTL_CAPTURE_CREATE_STREAM, &stream) == 0);
	malformed_requests(files.capture_fd, files.control_fd, readonly_fd, &destination);
	CHECK(close(readonly_fd) == 0);
	duplicate = fcntl(files.capture_fd, F_DUPFD_CLOEXEC, 0);
	CHECK(duplicate >= 0);
	CHECK(register_destination(duplicate, &destination, 1) == -1 && errno == ESTALE);
	for (uint64_t id = 2; id <= 16; id++)
		CHECK(register_destination(files.capture_fd, &destination, id) == 0);
	CHECK(register_destination(files.capture_fd, &destination, 17) == -1 && errno == EBUSY);
	CHECK(remove_destination(duplicate, 1) == 0);
	CHECK(register_destination(files.capture_fd, &destination, 17) == 0);
	for (uint64_t id = 2; id <= 16; id++)
		CHECK(remove_destination(files.capture_fd, id) == 0);
	CHECK(register_destination(files.capture_fd, &destination, UINT64_MAX) == 0);
	CHECK(register_destination(files.capture_fd, &destination, 18) == -1 && errno == EOVERFLOW);
	CHECK(close(files.control_fd) == 0);
	CHECK(register_destination(files.capture_fd, &destination, 18) == -1 && errno == EKEYREVOKED);
	CHECK(close(output_fd) == 0);
	/* Removing retained registrations does not need the original DMA-BUF descriptors. */
	CHECK(remove_destination(duplicate, 17) == 0);
	CHECK(remove_destination(files.capture_fd, UINT64_MAX) == 0);
	CHECK(remove_destination(files.capture_fd, UINT64_MAX) == -1 && errno == ENOENT);
	CHECK(drmIoctl(files.capture_fd, DRM_IOCTL_CAPTURE_DESTROY_STREAM, &close_stream) == 0);
	CHECK(close(duplicate) == 0);
	CHECK(close(files.capture_fd) == 0);
	/* A new client reuses local names and releases registrations on final close. */
	CHECK(drmIoctl(master, DRM_IOCTL_MODE_CREATE_CAPTURE_GRANT, &grant) == 0);
	CHECK(drmPrimeHandleToFD(master, output.dumb.handle, DRM_CLOEXEC | DRM_RDWR, &output_fd) == 0);
	destination.fds[0] = output_fd;
	CHECK(register_destination(files.capture_fd, &destination, 1) == 0);
	CHECK(close(output_fd) == 0);
	destroy_buffer(master, &output);
	CHECK(close(files.capture_fd) == 0);
	CHECK(close(files.control_fd) == 0);
	CHECK(drmModeSetCrtc(master, grant.crtc_id, 0, 0, 0, NULL, 0, NULL) == 0);
	destroy_buffer(master, &read_only);
	destroy_buffer(master, &scanout);
	CHECK(close(master) == 0);
	puts("PASS: capture destination admission, retained descriptors and revoked cleanup");
	return 0;
}
