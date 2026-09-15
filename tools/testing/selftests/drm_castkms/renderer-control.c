// SPDX-License-Identifier: GPL-2.0-only

/* Renderer capability publication and revocation, without source access. */
#include "fixture.h"

#include <dirent.h>
#include <drm_fourcc.h>
#include <fcntl.h>
#include <string.h>
#include <sys/ioctl.h>
#include <unistd.h>

#include "../../../../include/uapi/drm/castkms_drm.h"

_Static_assert(sizeof(struct drm_castkms_renderer_files) == 8,
	       "renderer file ABI");
_Static_assert(sizeof(struct drm_castkms_create_renderer_control) == 32,
	       "renderer creation ABI");
_Static_assert(sizeof(struct drm_castkms_renderer_query) == 24,
	       "renderer query ABI");

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

static struct drm_castkms_renderer_query query_renderer(int fd)
{
	struct drm_castkms_renderer_query query;

	memset(&query, 0xa5, sizeof(query));
	CHECK(ioctl(fd, DRM_IOCTL_CASTKMS_RENDERER_QUERY, &query) == 0);
	CHECK(query.version == DRM_CASTKMS_RENDERER_VERSION);
	CHECK(query.flags == 0);
	CHECK(query.profile == DRM_CASTKMS_EXECUTION_HOST_V1);
	CHECK(query.reserved == 0);
	CHECK(query.generation != 0);
	return query;
}

int main(int argc, char **argv)
{
	struct drm_castkms_create_renderer_control request = {};
	struct drm_castkms_renderer_files files;
	struct drm_castkms_renderer_query first, next;
	drmModeConnector *connector;
	drmModeRes *resources;
	struct buffer buffer;
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
	drmModeFreeConnector(connector);

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

	duplicate = fcntl(files.revoke_fd, F_DUPFD_CLOEXEC, 0);
	CHECK(duplicate >= 0);
	CHECK(close(files.revoke_fd) == 0);
	next = query_renderer(files.renderer_fd);
	CHECK(!memcmp(&first, &next, sizeof(first)));
	CHECK(close(duplicate) == 0);
	expect_ioctl_error(files.renderer_fd, DRM_IOCTL_CASTKMS_RENDERER_QUERY,
			   &next, EKEYREVOKED);
	CHECK(close(files.renderer_fd) == 0);

	files = create_renderer(fd, &request);
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
	query_renderer(files.renderer_fd);
	CHECK(close(files.renderer_fd) == 0);
	CHECK(close(files.revoke_fd) == 0);
	destroy_buffer(fd, &buffer);
	CHECK(close(peer) == 0);
	CHECK(close(fd) == 0);
	puts("PASS: renderer capability publication, query and revocation");
	return 0;
}
