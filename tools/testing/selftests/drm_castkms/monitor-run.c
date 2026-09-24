// SPDX-License-Identifier: GPL-2.0-only

/* Keep a virtual monitor attached while an independent DRM-master client runs. */
#include "fixture.h"

#include <fcntl.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

int main(int argc, char **argv)
{
	drmModeRes *resources;
	struct monitor_control monitor;
	drmVersion *version;
	pid_t child;
	int fd, status;

	if (argc < 3) {
		fprintf(stderr, "usage: monitor-run /dev/dri/cardN command [args...]\n");
		return 2;
	}
	fd = open(argv[1], O_RDWR | O_CLOEXEC);
	CHECK(fd >= 0);
	version = drmGetVersion(fd);
	CHECK(version && !strcmp(version->name, "castkms"));
	drmFreeVersion(version);
	CHECK(drmIsMaster(fd));
	resources = drmModeGetResources(fd);
	CHECK(resources && resources->count_connectors > 0);
	monitor = attach_fallback_monitor(fd, resources->connectors[0]);
	drmModeFreeResources(resources);
	CHECK(drmDropMaster(fd) == 0);
	child = fork();
	CHECK(child >= 0);
	if (!child) {
		close(monitor.control_fd);
		close(monitor.revoke_fd);
		close(fd);
		execvp(argv[2], &argv[2]);
		perror("execvp");
		_exit(127);
	}
	while (waitpid(child, &status, 0) < 0)
		CHECK(errno == EINTR);
	close_monitor(&monitor);
	CHECK(close(fd) == 0);
	if (WIFEXITED(status))
		return WEXITSTATUS(status);
	if (WIFSIGNALED(status))
		return 128 + WTERMSIG(status);
	return 1;
}
