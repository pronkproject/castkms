// SPDX-License-Identifier: GPL-2.0-only
/* Exercise the exclusive virtual-monitor capability through real DRM files. */
#include <fcntl.h>
#include <dirent.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <unistd.h>

#include "fixture.h"

#include "../../../../include/uapi/drm/castkms_drm.h"

_Static_assert(sizeof(struct drm_castkms_create_monitor_control) == 32,
	       "monitor request layout");
_Static_assert(sizeof(struct drm_castkms_monitor_files) == 8,
	       "monitor result layout");

static const unsigned char edid_1080p[128] = {
	0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00,
	0x31, 0xd8, 0x2a, 0x00, 0x00, 0x00, 0x00, 0x00,
	0x00, 0x21, 0x01, 0x03, 0x81, 0xa0, 0x5a, 0x78,
	0x0a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
	0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x01,
	0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
	0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x02, 0x3a,
	0x80, 0x18, 0x71, 0x38, 0x2d, 0x40, 0x58, 0x2c,
	0x45, 0x00, 0x40, 0x84, 0x63, 0x00, 0x00, 0x1e,
	0x00, 0x00, 0x00, 0xfc, 0x00, 0x54, 0x65, 0x73,
	0x74, 0x20, 0x45, 0x44, 0x49, 0x44, 0x0a, 0x20,
	0x20, 0x20, 0x00, 0x00, 0x00, 0xfd, 0x00, 0x32,
	0x46, 0x1e, 0x46, 0x0f, 0x00, 0x0a, 0x20, 0x20,
	0x20, 0x20, 0x20, 0x20, 0x00, 0x00, 0x00, 0x10,
	0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
	0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xab,
};

static drmModeConnector *probe_connector(int fd, uint32_t connector_id)
{
	drmModeConnector *connector = drmModeGetConnector(fd, connector_id);

	CHECK(connector);
	return connector;
}

static void expect_connection(int fd, uint32_t connector_id,
			      drmModeConnection connection)
{
	drmModeConnector *connector = probe_connector(fd, connector_id);

	CHECK(connector->connection == connection);
	if (connection == DRM_MODE_CONNECTED)
		CHECK(connector->count_modes > 0);
	else
		CHECK(connector->count_modes == 0);
	drmModeFreeConnector(connector);
}

static void expect_1080p_fallback(int fd, uint32_t connector_id)
{
	drmModeConnector *connector = probe_connector(fd, connector_id);
	bool found = false;

	for (int i = 0; i < connector->count_modes; i++) {
		drmModeModeInfo *mode = &connector->modes[i];

		if (mode->hdisplay == 1920 && mode->vdisplay == 1080
		    && (mode->type & DRM_MODE_TYPE_PREFERRED))
			found = true;
	}
	CHECK(found);
	drmModeFreeConnector(connector);
}

static int create_control(int fd, uint32_t connector_id, int *revoke_fd)
{
	struct drm_castkms_monitor_files files;
	struct drm_castkms_create_monitor_control request = {
		.connector_id = connector_id,
		.files = (uintptr_t)&files,
	};

	CHECK(drmIoctl(fd, DRM_IOCTL_CASTKMS_CREATE_MONITOR_CONTROL,
		       &request) == 0);
	CHECK(files.control_fd >= 0);
	CHECK(files.revoke_fd >= 0);
	CHECK(files.control_fd != files.revoke_fd);
	*revoke_fd = files.revoke_fd;
	return files.control_fd;
}

static unsigned int open_files(void)
{
	DIR *directory = opendir("/proc/self/fd");
	struct dirent *entry;
	unsigned int count = 0;

	CHECK(directory);
	while ((entry = readdir(directory)))
		if (entry->d_name[0] != '.')
			count++;
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

int main(int argc, char **argv)
{
	struct drm_castkms_monitor_files files;
	struct drm_castkms_create_monitor_control create = {
		.files = (uintptr_t)&files,
	};
	struct drm_castkms_monitor_query query = {0};
	struct drm_castkms_monitor_attach attach = {0};
	struct drm_castkms_monitor_detach detach = {0};
	drmModeRes *resources;
	unsigned char invalid_edid[sizeof(edid_1080p)];
	uint32_t connector_id;
	int control, fd, peer, revoke;

	if (argc != 2) {
		fprintf(stderr, "SKIP: supply a disposable Rust CastKMS DRM node\n");
		return 4;
	}
	fd = open(argv[1], O_RDWR | O_CLOEXEC);
	CHECK(fd >= 0);
	CHECK(drmSetMaster(fd) == 0);
	peer = open(argv[1], O_RDWR | O_CLOEXEC);
	CHECK(peer >= 0 && drmIsMaster(peer) == 0);
	resources = drmModeGetResources(fd);
	CHECK(resources && resources->count_connectors > 0);
	connector_id = resources->connectors[0];
	drmModeFreeResources(resources);

	create.connector_id = connector_id;
	create.flags = DRM_CASTKMS_MONITOR_CREATE_ADMIN << 1;
	expect_ioctl_error(fd, DRM_IOCTL_CASTKMS_CREATE_MONITOR_CONTROL,
			   &create, EINVAL);
	create.flags = 0;
	create.reserved[0] = 1;
	expect_ioctl_error(fd, DRM_IOCTL_CASTKMS_CREATE_MONITOR_CONTROL,
			   &create, EINVAL);
	create.reserved[0] = 0;
	expect_ioctl_error(peer, DRM_IOCTL_CASTKMS_CREATE_MONITOR_CONTROL,
			   &create, EACCES);

	/* Failed result copyout must leave both fd table and connection state intact. */
	unsigned int before = open_files();
	create.files = 1;
	for (int i = 0; i < 4; i++) {
		expect_ioctl_error(fd, DRM_IOCTL_CASTKMS_CREATE_MONITOR_CONTROL,
				   &create, EFAULT);
		CHECK(open_files() == before);
		expect_connection(fd, connector_id, DRM_MODE_DISCONNECTED);
	}
	create.files = (uintptr_t)&files;
	/* The request is input-only; only its explicit result pointer is written. */
	long page_size = sysconf(_SC_PAGESIZE);
	void *partial = mmap(NULL, 2 * page_size, PROT_READ | PROT_WRITE,
		MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	CHECK(partial != MAP_FAILED);
	CHECK(mprotect((char *)partial + page_size, page_size, PROT_NONE) == 0);
	create.files = (uintptr_t)((char *)partial + page_size - sizeof(int32_t));
	expect_ioctl_error(fd, DRM_IOCTL_CASTKMS_CREATE_MONITOR_CONTROL,
			   &create, EFAULT);
	CHECK(open_files() == before);
	expect_connection(fd, connector_id, DRM_MODE_DISCONNECTED);
	CHECK(munmap(partial, 2 * page_size) == 0);
	create.files = (uintptr_t)&files;
	struct drm_castkms_create_monitor_control *readonly = mmap(NULL, page_size,
		PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	CHECK(readonly != MAP_FAILED);
	*readonly = create;
	CHECK(mprotect(readonly, page_size, PROT_READ) == 0);
	CHECK(ioctl(fd, DRM_IOCTL_CASTKMS_CREATE_MONITOR_CONTROL, readonly) == 0);
	CHECK(close(files.control_fd) == 0 && close(files.revoke_fd) == 0);
	CHECK(munmap(readonly, page_size) == 0);
	CHECK(open_files() == before);
	expect_connection(fd, connector_id, DRM_MODE_DISCONNECTED);

	control = create_control(fd, connector_id, &revoke);
	CHECK(fcntl(control, F_GETFD) == FD_CLOEXEC);
	CHECK(fcntl(revoke, F_GETFD) == FD_CLOEXEC);
	CHECK((fcntl(control, F_GETFL) & O_ACCMODE) == O_RDWR);
	expect_connection(fd, connector_id, DRM_MODE_DISCONNECTED);
	expect_ioctl_error(control, _IO('x', 0xff), NULL, ENOTTY);
	expect_ioctl_error(control, DRM_IOCTL_CASTKMS_MONITOR_QUERY, NULL, EFAULT);
	CHECK(ioctl(control, DRM_IOCTL_CASTKMS_MONITOR_QUERY, &query) == 0);
	CHECK(query.version == DRM_CASTKMS_MONITOR_CONTROL_VERSION);
	CHECK(query.flags == DRM_CASTKMS_MONITOR_CAP_CEC);
	CHECK(query.max_edid_size == DRM_CASTKMS_MONITOR_MAX_EDID_SIZE);
	CHECK(query.reserved == 0);

	attach.edid_ptr = (uintptr_t)edid_1080p;
	attach.edid_size = sizeof(edid_1080p);
	attach.flags = 1;
	expect_ioctl_error(control, DRM_IOCTL_CASTKMS_MONITOR_ATTACH,
			   &attach, EINVAL);
	attach.flags = 0;
	attach.edid_size = 0;
	expect_ioctl_error(control, DRM_IOCTL_CASTKMS_MONITOR_ATTACH,
			   &attach, EINVAL);
	attach.edid_ptr = 0;
	attach.edid_size = 1;
	expect_ioctl_error(control, DRM_IOCTL_CASTKMS_MONITOR_ATTACH,
			   &attach, EINVAL);
	attach.edid_ptr = 1;
	attach.edid_size = sizeof(edid_1080p);
	expect_ioctl_error(control, DRM_IOCTL_CASTKMS_MONITOR_ATTACH,
			   &attach, EFAULT);
	memcpy(invalid_edid, edid_1080p, sizeof(invalid_edid));
	invalid_edid[127] ^= 1;
	attach.edid_ptr = (uintptr_t)invalid_edid;
	expect_ioctl_error(control, DRM_IOCTL_CASTKMS_MONITOR_ATTACH,
			   &attach, EINVAL);

	attach.edid_ptr = (uintptr_t)edid_1080p;
	CHECK(ioctl(control, DRM_IOCTL_CASTKMS_MONITOR_ATTACH, &attach) == 0);
	expect_connection(fd, connector_id, DRM_MODE_CONNECTED);
	expect_ioctl_error(fd, DRM_IOCTL_CASTKMS_CREATE_MONITOR_CONTROL,
			   &create, EBUSY);
	CHECK(drmDropMaster(fd) == 0);
	acquire_master(peer);

	detach.flags = 1;
	expect_ioctl_error(control, DRM_IOCTL_CASTKMS_MONITOR_DETACH,
			   &detach, EINVAL);
	detach.flags = 0;
	detach.reserved = 1;
	expect_ioctl_error(control, DRM_IOCTL_CASTKMS_MONITOR_DETACH,
			   &detach, EINVAL);
	detach.reserved = 0;
	CHECK(ioctl(control, DRM_IOCTL_CASTKMS_MONITOR_DETACH, &detach) == 0);
	expect_connection(peer, connector_id, DRM_MODE_DISCONNECTED);

	CHECK(close(revoke) == 0);
	attach.edid_size = 0;
	attach.edid_ptr = 0;
	expect_ioctl_error(control, DRM_IOCTL_CASTKMS_MONITOR_ATTACH,
			   &attach, ECANCELED);
	expect_connection(peer, connector_id, DRM_MODE_DISCONNECTED);
	CHECK(close(control) == 0);
	control = create_control(peer, connector_id, &revoke);
	CHECK(ioctl(control, DRM_IOCTL_CASTKMS_MONITOR_ATTACH, &attach) == 0);
	expect_connection(peer, connector_id, DRM_MODE_CONNECTED);
	expect_1080p_fallback(peer, connector_id);
	CHECK(close(control) == 0);
	expect_connection(peer, connector_id, DRM_MODE_DISCONNECTED);
	expect_ioctl_error(revoke, _IO('x', 0xff), NULL, ENOTTY);
	CHECK(close(revoke) == 0);
	CHECK(close(peer) == 0);
	CHECK(close(fd) == 0);
	puts("PASS: explicit CastKMS monitor publication and 1080p fallback");
	return 0;
}
