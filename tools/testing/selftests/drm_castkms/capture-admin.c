// SPDX-License-Identifier: GPL-2.0-only

/* Administrative final-image capture issuance without transferring DRM master. */
#include "fixture.h"

#include <fcntl.h>
#include <sys/ioctl.h>
#include <unistd.h>

#include "../../../../include/uapi/drm/drm_capture.h"

static void expect_error(int fd, unsigned long command, void *request, int error)
{
	errno = 0;
	CHECK(ioctl(fd, command, request) < 0);
	CHECK(errno == error);
}

static void close_files(struct drm_capture_grant_files *files)
{
	CHECK(close(files->capture_fd) == 0);
	CHECK(close(files->control_fd) == 0);
	files->capture_fd = -1;
	files->control_fd = -1;
}

int main(int argc, char **argv)
{
	struct drm_capture_grant_files files = {
		.capture_fd = -1,
		.control_fd = -1,
	};
	struct drm_mode_create_capture_grant create = {
		.files = (uintptr_t)&files,
	};
	struct drm_capture_describe describe = { 0 };
	drmModeRes *resources;
	uint64_t capability;
	int master, helper;

	CHECK(argc == 2);
	master = open(argv[1], O_RDWR | O_CLOEXEC);
	CHECK(master >= 0 && drmIsMaster(master));
	CHECK(drmGetCap(master, DRM_CAP_CAPTURE_GRANT, &capability) == 0 && capability == 1);
	CHECK(drmGetCap(master, DRM_CAP_CAPTURE_GRANT_ADMIN, &capability) == 0 && capability == 1);
	helper = open(argv[1], O_RDWR | O_CLOEXEC);
	CHECK(helper >= 0 && !drmIsMaster(helper));
	resources = drmModeGetResources(master);
	CHECK(resources && resources->count_crtcs > 0 && resources->count_connectors > 0);
	create.crtc_id = resources->crtcs[0];
	create.connector_id = resources->connectors[0];

	expect_error(helper, DRM_IOCTL_MODE_CREATE_CAPTURE_GRANT, &create, EACCES);
	create.flags = 2;
	expect_error(helper, DRM_IOCTL_MODE_CREATE_CAPTURE_GRANT, &create, EINVAL);
	create.flags = DRM_CAPTURE_GRANT_CREATE_ADMIN;
	expect_error(master, DRM_IOCTL_MODE_CREATE_CAPTURE_GRANT, &create, EBUSY);
	CHECK(ioctl(helper, DRM_IOCTL_MODE_CREATE_CAPTURE_GRANT, &create) == 0);
	CHECK(fcntl(files.capture_fd, F_GETFD) == FD_CLOEXEC);
	CHECK(fcntl(files.control_fd, F_GETFD) == FD_CLOEXEC);
	CHECK(close(helper) == 0);
	expect_error(files.capture_fd, DRM_IOCTL_CAPTURE_DESCRIBE, &describe, EKEYREVOKED);
	close_files(&files);

	helper = open(argv[1], O_RDWR | O_CLOEXEC);
	CHECK(helper >= 0 && !drmIsMaster(helper));
	CHECK(ioctl(helper, DRM_IOCTL_MODE_CREATE_CAPTURE_GRANT, &create) == 0);
	expect_error(files.capture_fd, DRM_IOCTL_CAPTURE_DESCRIBE, &describe, ENODEV);
	CHECK(drmDropMaster(master) == 0);
	expect_error(files.capture_fd, DRM_IOCTL_CAPTURE_DESCRIBE, &describe, ESTALE);
	CHECK(drmSetMaster(master) == 0);
	expect_error(files.capture_fd, DRM_IOCTL_CAPTURE_DESCRIBE, &describe, ESTALE);

	close_files(&files);
	CHECK(close(helper) == 0);
	drmModeFreeResources(resources);
	CHECK(close(master) == 0);
	return 0;
}
