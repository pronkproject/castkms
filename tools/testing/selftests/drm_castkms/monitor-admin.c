// SPDX-License-Identifier: GPL-2.0-only

/* Administrative monitor issuance without transferring DRM master. */
#include "fixture.h"

#include <fcntl.h>
#include <sys/ioctl.h>
#include <unistd.h>

#include "../../../../include/uapi/drm/castkms_drm.h"

static void expect_connection(int fd, uint32_t connector_id,
			      drmModeConnection expected)
{
	drmModeConnector *connector = drmModeGetConnector(fd, connector_id);

	CHECK(connector);
	CHECK(connector->connection == expected);
	CHECK((connector->count_modes > 0) == (expected == DRM_MODE_CONNECTED));
	drmModeFreeConnector(connector);
}

static void expect_error(int fd, unsigned long command, void *request, int error)
{
	errno = 0;
	CHECK(ioctl(fd, command, request) < 0);
	CHECK(errno == error);
}

static struct drm_castkms_monitor_files create_control(int fd,
						       uint32_t connector_id)
{
	struct drm_castkms_monitor_files files = { .control_fd = -1,
						    .revoke_fd = -1 };
	struct drm_castkms_create_monitor_control create = {
		.connector_id = connector_id,
		.flags = DRM_CASTKMS_MONITOR_CREATE_ADMIN,
		.files = (uintptr_t)&files,
	};

	CHECK(ioctl(fd, DRM_IOCTL_CASTKMS_CREATE_MONITOR_CONTROL, &create) == 0);
	CHECK(files.control_fd >= 0 && files.revoke_fd >= 0);
	CHECK(files.control_fd != files.revoke_fd);
	CHECK(fcntl(files.control_fd, F_GETFD) == FD_CLOEXEC);
	CHECK(fcntl(files.revoke_fd, F_GETFD) == FD_CLOEXEC);
	return files;
}

static void close_control(struct drm_castkms_monitor_files *files)
{
	CHECK(close(files->control_fd) == 0);
	CHECK(close(files->revoke_fd) == 0);
}

int main(int argc, char **argv)
{
	struct drm_castkms_monitor_attach attach = { 0 };
	struct drm_castkms_monitor_files files;
	struct drm_castkms_create_monitor_control invalid = {
		.flags = DRM_CASTKMS_MONITOR_CREATE_ADMIN << 1,
	};
	drmModeRes *resources;
	uint32_t connector_id;
	int helper, master;

	CHECK(argc == 2);
	master = open(argv[1], O_RDWR | O_CLOEXEC);
	CHECK(master >= 0 && drmIsMaster(master));
	helper = open(argv[1], O_RDWR | O_CLOEXEC);
	CHECK(helper >= 0 && !drmIsMaster(helper));
	resources = drmModeGetResources(master);
	CHECK(resources && resources->count_connectors > 0);
	connector_id = resources->connectors[0];
	drmModeFreeResources(resources);

	invalid.connector_id = connector_id;
	expect_error(helper, DRM_IOCTL_CASTKMS_CREATE_MONITOR_CONTROL,
		     &invalid, EINVAL);

	/* Administration neither requires nor displaces the current master. */
	files = create_control(helper, connector_id);
	CHECK(drmIsMaster(master));
	CHECK(!drmIsMaster(helper));
	CHECK(ioctl(files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_ATTACH,
		    &attach) == 0);
	expect_connection(master, connector_id, DRM_MODE_CONNECTED);
	close_control(&files);
	expect_connection(master, connector_id, DRM_MODE_DISCONNECTED);

	/* The same attachment-only authority is available before a compositor. */
	CHECK(drmDropMaster(master) == 0);
	CHECK(!drmIsMaster(master) && !drmIsMaster(helper));
	files = create_control(helper, connector_id);
	CHECK(ioctl(files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_ATTACH,
		    &attach) == 0);
	acquire_master(master);
	expect_connection(master, connector_id, DRM_MODE_CONNECTED);
	CHECK(close(files.revoke_fd) == 0);
	expect_connection(master, connector_id, DRM_MODE_DISCONNECTED);
	expect_error(files.control_fd, DRM_IOCTL_CASTKMS_MONITOR_ATTACH,
		     &attach, ECANCELED);
	CHECK(close(files.control_fd) == 0);

	CHECK(close(helper) == 0);
	CHECK(close(master) == 0);
	puts("PASS: administrative monitor publication preserves DRM ownership");
	return 0;
}
