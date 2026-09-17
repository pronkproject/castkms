// SPDX-License-Identifier: GPL-2.0-only

/* Administrative renderer issuance without transferring DRM master. */
#include "fixture.h"

#include <fcntl.h>
#include <sys/ioctl.h>
#include <unistd.h>

#include "../../../../include/uapi/drm/castkms_drm.h"

static void expect_error(int fd, unsigned long command, void *request, int error)
{
	errno = 0;
	CHECK(ioctl(fd, command, request) < 0);
	CHECK(errno == error);
}

static void close_files(struct drm_castkms_renderer_files *files)
{
	CHECK(close(files->renderer_fd) == 0);
	CHECK(close(files->revoke_fd) == 0);
	files->renderer_fd = -1;
	files->revoke_fd = -1;
}

int main(int argc, char **argv)
{
	struct drm_castkms_renderer_files files = {
		.renderer_fd = -1,
		.revoke_fd = -1,
	};
	struct drm_castkms_create_renderer create = {
		.files = (uintptr_t)&files,
	};
	struct drm_castkms_renderer_query query = { 0 };
	drmModeRes *resources;
	int master, helper;

	CHECK(argc == 2);
	master = open(argv[1], O_RDWR | O_CLOEXEC);
	CHECK(master >= 0 && drmIsMaster(master));
	helper = open(argv[1], O_RDWR | O_CLOEXEC);
	CHECK(helper >= 0 && !drmIsMaster(helper));
	resources = drmModeGetResources(master);
	CHECK(resources && resources->count_crtcs > 0 && resources->count_connectors > 0);
	create.crtc_id = resources->crtcs[0];
	create.connector_id = resources->connectors[0];

	expect_error(helper, DRM_IOCTL_CASTKMS_CREATE_RENDERER, &create, EACCES);
	create.flags = 2;
	expect_error(helper, DRM_IOCTL_CASTKMS_CREATE_RENDERER, &create, EINVAL);
	create.flags = DRM_CASTKMS_RENDERER_CREATE_ADMIN;
	expect_error(master, DRM_IOCTL_CASTKMS_CREATE_RENDERER, &create, EBUSY);
	CHECK(ioctl(helper, DRM_IOCTL_CASTKMS_CREATE_RENDERER, &create) == 0);
	CHECK(fcntl(files.renderer_fd, F_GETFD) == FD_CLOEXEC);
	CHECK(fcntl(files.revoke_fd, F_GETFD) == FD_CLOEXEC);
	CHECK(close(helper) == 0);
	expect_error(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_QUERY,
		     &query, EKEYREVOKED);
	close_files(&files);

	helper = open(argv[1], O_RDWR | O_CLOEXEC);
	CHECK(helper >= 0 && !drmIsMaster(helper));
	CHECK(ioctl(helper, DRM_IOCTL_CASTKMS_CREATE_RENDERER, &create) == 0);
	CHECK(ioctl(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_QUERY, &query) == 0);
	CHECK(query.version == DRM_CASTKMS_RENDERER_VERSION);
	CHECK(drmDropMaster(master) == 0);
	expect_error(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_QUERY, &query, ESTALE);
	CHECK(drmSetMaster(master) == 0);
	expect_error(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_QUERY, &query, ESTALE);

	close_files(&files);
	CHECK(close(helper) == 0);
	drmModeFreeResources(resources);
	CHECK(close(master) == 0);
	return 0;
}
