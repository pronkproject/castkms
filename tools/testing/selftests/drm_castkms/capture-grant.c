// SPDX-License-Identifier: GPL-2.0-only

/* Grant publication and revocation, without requesting or reading pixels. */
#include "fixture.h"

#include <dirent.h>
#include <drm_fourcc.h>
#include <fcntl.h>
#include <poll.h>
#include <string.h>
#include <sys/mman.h>
#include <sys/resource.h>
#include <unistd.h>

#include "../../../../include/uapi/drm/drm_capture.h"

_Static_assert(sizeof(struct drm_capture_describe) == 48, "capture description ABI");

static unsigned int open_files(int *highest)
{
	DIR *directory = opendir("/proc/self/fd");
	struct dirent *entry;
	unsigned int count = 0;

	CHECK(directory);
	*highest = -1;
	while ((entry = readdir(directory))) {
		char *end;
		long fd = strtol(entry->d_name, &end, 10);

		if (*end || fd < 0 || fd == dirfd(directory))
			continue;
		count++;
		if (fd > *highest)
			*highest = fd;
	}
	CHECK(closedir(directory) == 0);
	return count;
}

static int revoked(int fd)
{
	struct pollfd event = { .fd = fd, .events = POLLIN };

	CHECK(poll(&event, 1, 0) >= 0);
	CHECK(!(event.revents & (POLLERR | POLLNVAL)));
	return !!(event.revents & POLLHUP);
}

static void rejected_publication(int fd, struct drm_mode_create_capture_grant request)
{
	struct drm_capture_grant_files files = { -1, -1 };
	struct rlimit saved, limited;
	unsigned int before;
	long page_size = sysconf(_SC_PAGESIZE);
	void *partial;
	int highest, probe;

	before = open_files(&highest);
	request.files = 1;
	/* More failed grants than the creator quota must neither leak fds nor consume quota. */
	for (unsigned int i = 0; i < 128; i++) {
		CHECK(drmIoctl(fd, DRM_IOCTL_MODE_CREATE_CAPTURE_GRANT, &request) == -1);
		CHECK(errno == EFAULT);
	}
	CHECK(open_files(&highest) == before);
	CHECK(page_size > 0);
	partial = mmap(NULL, page_size * 2, PROT_READ | PROT_WRITE,
		       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	CHECK(partial != MAP_FAILED);
	CHECK(mprotect((char *)partial + page_size, page_size, PROT_NONE) == 0);
	request.files = (uintptr_t)((char *)partial + page_size - sizeof(files.capture_fd));
	CHECK(drmIoctl(fd, DRM_IOCTL_MODE_CREATE_CAPTURE_GRANT, &request) == -1);
	CHECK(errno == EFAULT);
	CHECK(open_files(&highest) == before);
	CHECK(munmap(partial, page_size * 2) == 0);
	request.files = (uintptr_t)&files;
	CHECK(getrlimit(RLIMIT_NOFILE, &saved) == 0);
	probe = fcntl(fd, F_DUPFD_CLOEXEC, 0);
	CHECK(probe >= 0);
	CHECK(close(probe) == 0);
	limited = saved;
	/* Leave one descriptor slot: the second reservation must roll back the first. */
	limited.rlim_cur = probe + 1;
	CHECK(limited.rlim_cur < saved.rlim_cur);
	CHECK(setrlimit(RLIMIT_NOFILE, &limited) == 0);
	CHECK(drmIoctl(fd, DRM_IOCTL_MODE_CREATE_CAPTURE_GRANT, &request) == -1);
	CHECK(errno == EMFILE);
	CHECK(setrlimit(RLIMIT_NOFILE, &saved) == 0);
	CHECK(open_files(&highest) == before);
	CHECK(files.capture_fd == -1 && files.control_fd == -1);
	request.flags = 1;
	CHECK(drmIoctl(fd, DRM_IOCTL_MODE_CREATE_CAPTURE_GRANT, &request) == -1);
	CHECK(errno == EINVAL);
	request.flags = 0;
	for (unsigned int i = 0; i < 3; i++) {
		request.reserved[i] = 1;
		CHECK(drmIoctl(fd, DRM_IOCTL_MODE_CREATE_CAPTURE_GRANT, &request) == -1);
		CHECK(errno == EINVAL);
		request.reserved[i] = 0;
	}
	request.crtc_id = request.connector_id;
	CHECK(drmIoctl(fd, DRM_IOCTL_MODE_CREATE_CAPTURE_GRANT, &request) == -1);
	CHECK(errno == ENOENT);
	CHECK(open_files(&highest) == before);
}

static void describe_output(int master, const struct drm_mode_create_capture_grant *request,
			   const struct drm_capture_grant_files *files)
{
	struct drm_capture_describe first, next;
	drmModeConnector *connector = drmModeGetConnector(master, request->connector_id);
	drmModeModeInfo mode;
	struct buffer buffer;
	uint32_t connector_id = request->connector_id;
	unsigned int before;
	int highest;
	long page_size = sysconf(_SC_PAGESIZE);
	void *partial;

	CHECK(connector && connector->count_modes > 0 && page_size > 0);
	mode = connector->modes[0];
	drmModeFreeConnector(connector);
	CHECK(drmIoctl(files->capture_fd, DRM_IOCTL_CAPTURE_DESCRIBE, &first) == -1);
	CHECK(errno == ENODEV);
	buffer = create_buffer(master, mode.hdisplay, mode.vdisplay, 0x39);
	CHECK(drmModeSetCrtc(master, request->crtc_id, buffer.fb, 0, 0,
			     &connector_id, 1, &mode) == 0);
	memset(&first, 0xa5, sizeof(first));
	CHECK(drmIoctl(files->capture_fd, DRM_IOCTL_CAPTURE_DESCRIBE, &first) == 0);
	CHECK(first.id == 1 && first.width == mode.hdisplay && first.height == mode.vdisplay);
	CHECK(first.format == DRM_FORMAT_XRGB8888 && first.modifier == DRM_FORMAT_MOD_LINEAR);
	CHECK(first.max_requests == 8 && first.reserved[0] == 0 && first.reserved[1] == 0);
	CHECK(drmIoctl(files->control_fd, DRM_IOCTL_CAPTURE_DESCRIBE, &next) == -1);
	CHECK(errno == ENOTTY);
	CHECK(drmIoctl(files->capture_fd, DRM_IOCTL_CAPTURE_DESCRIBE ^ (1U << _IOC_SIZESHIFT),
		       &next) == -1);
	CHECK(errno == ENOTTY);
	before = open_files(&highest);
	for (unsigned int i = 0; i < 128; i++) {
		CHECK(drmIoctl(files->capture_fd, DRM_IOCTL_CAPTURE_DESCRIBE, (void *)1) == -1);
		CHECK(errno == EFAULT);
	}
	partial = mmap(NULL, page_size * 2, PROT_READ | PROT_WRITE,
		       MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	CHECK(partial != MAP_FAILED);
	CHECK(mprotect((char *)partial + page_size, page_size, PROT_NONE) == 0);
	CHECK(drmIoctl(files->capture_fd, DRM_IOCTL_CAPTURE_DESCRIBE,
		       (char *)partial + page_size - sizeof(first.id)) == -1);
	CHECK(errno == EFAULT);
	CHECK(munmap(partial, page_size * 2) == 0);
	CHECK(open_files(&highest) == before);
	CHECK(drmIoctl(files->capture_fd, DRM_IOCTL_CAPTURE_DESCRIBE, &next) == 0);
	CHECK(!memcmp(&first, &next, sizeof(first)));
	/* Changed timings create another mode interval without changing visible geometry. */
	mode.clock++;
	CHECK(drmModeSetCrtc(master, request->crtc_id, buffer.fb, 0, 0,
			     &connector_id, 1, &mode) == 0);
	CHECK(drmIoctl(files->capture_fd, DRM_IOCTL_CAPTURE_DESCRIBE, &next) == 0);
	CHECK(next.id == first.id + 1 && next.width == first.width && next.height == first.height);
	CHECK(drmModeSetCrtc(master, request->crtc_id, 0, 0, 0, NULL, 0, NULL) == 0);
	CHECK(drmIoctl(files->capture_fd, DRM_IOCTL_CAPTURE_DESCRIBE, &next) == -1);
	CHECK(errno == ENODEV);
	destroy_buffer(master, &buffer);
}

int main(int argc, char **argv)
{
	struct drm_mode_create_capture_grant request = {}, *read_only;
	struct drm_capture_grant_files files;
	struct drm_capture_describe description;
	struct drm_mode_create_dumb dumb = {};
	drmVersion *version;
	drmModeRes *resources;
	uint64_t capability;
	long page_size = sysconf(_SC_PAGESIZE);
	int master, reader, duplicate;

	CHECK(argc == 2 && page_size > 0);
	master = open(argv[1], O_RDWR | O_CLOEXEC);
	CHECK(master >= 0 && drmIsMaster(master) == 1);
	version = drmGetVersion(master);
	CHECK(version && !strcmp(version->name, "castkms"));
	drmFreeVersion(version);
	CHECK(drmGetCap(master, DRM_CAP_CAPTURE_GRANT, &capability) == 0 && capability == 1);
	resources = drmModeGetResources(master);
	CHECK(resources && resources->count_crtcs == 1 && resources->count_connectors == 1);
	request.crtc_id = resources->crtcs[0];
	request.connector_id = resources->connectors[0];
	request.files = (uintptr_t)&files;
	drmModeFreeResources(resources);
	reader = open(argv[1], O_RDONLY | O_CLOEXEC);
	CHECK(reader >= 0 && !drmIsMaster(reader));
	CHECK(drmIoctl(reader, DRM_IOCTL_MODE_CREATE_CAPTURE_GRANT, &request) == -1);
	CHECK(errno == EACCES);
	rejected_publication(master, request);

	read_only = mmap(NULL, page_size, PROT_READ | PROT_WRITE,
			 MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
	CHECK(read_only != MAP_FAILED);
	*read_only = request;
	CHECK(mprotect(read_only, page_size, PROT_READ) == 0);
	/* The request is input-only; publication writes solely to the explicit output. */
	CHECK(drmIoctl(master, DRM_IOCTL_MODE_CREATE_CAPTURE_GRANT, read_only) == 0);
	CHECK(munmap(read_only, page_size) == 0);
	CHECK(files.capture_fd >= 0 && files.control_fd >= 0);
	CHECK(files.capture_fd != files.control_fd);
	CHECK(fcntl(files.capture_fd, F_GETFD) == FD_CLOEXEC);
	CHECK(fcntl(files.control_fd, F_GETFD) == FD_CLOEXEC);
	CHECK(drmIoctl(files.capture_fd, DRM_IOCTL_MODE_CREATE_DUMB, &dumb) == -1);
	CHECK(errno == ENOTTY);
	CHECK(!revoked(files.capture_fd) && !revoked(files.control_fd));
	describe_output(master, &request, &files);
	duplicate = fcntl(files.control_fd, F_DUPFD_CLOEXEC, 0);
	CHECK(duplicate >= 0);
	CHECK(close(files.control_fd) == 0);
	CHECK(!revoked(files.capture_fd));
	CHECK(close(duplicate) == 0);
	CHECK(revoked(files.capture_fd));
	CHECK(drmIoctl(files.capture_fd, DRM_IOCTL_CAPTURE_DESCRIBE, &description) == -1);
	CHECK(errno == EKEYREVOKED);
	CHECK(close(files.capture_fd) == 0);

	CHECK(drmIoctl(master, DRM_IOCTL_MODE_CREATE_CAPTURE_GRANT, &request) == 0);
	CHECK(close(files.capture_fd) == 0);
	CHECK(!revoked(files.control_fd));
	CHECK(close(master) == 0);
	CHECK(revoked(files.control_fd));
	CHECK(close(files.control_fd) == 0);
	CHECK(close(reader) == 0);
	puts("PASS: capture grant publication, descriptions, rollback and revocation");
	return 0;
}
