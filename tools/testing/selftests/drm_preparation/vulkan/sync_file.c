// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/sync_file.h>
#include <poll.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/ioctl.h>

#include "sync_file.h"

int gpu_sync_file_check(int fd, int timeout_ms)
{
	struct sync_file_info file = { 0 };
	struct sync_fence_info *fences;
	struct pollfd poll_fd = { .fd = fd, .events = POLLIN };
	unsigned int i, count;
	int result = -1;

	if (fd == -1) {
		puts("native completion: Vulkan already-completed sentinel");
		return 0;
	}
	if (fd < 0 || timeout_ms < 0)
		return -1;
	if (poll(&poll_fd, 1, timeout_ms) != 1 || !(poll_fd.revents & POLLIN)) {
		fprintf(stderr, "Native completion did not become readable (events=0x%x)\n",
			poll_fd.revents);
		return -1;
	}
	if (ioctl(fd, SYNC_IOC_FILE_INFO, &file) || file.status != 1 || !file.num_fences) {
		fprintf(stderr, "Native completion is not successful (status=%d)\n", file.status);
		return -1;
	}
	count = file.num_fences;
	fences = calloc(count, sizeof(*fences));
	if (!fences)
		return -1;
	file.sync_fence_info = (uintptr_t)fences;
	if (ioctl(fd, SYNC_IOC_FILE_INFO, &file) || file.status != 1 || file.num_fences != count)
		goto out;
	for (i = 0; i < count; i++) {
		printf("native fence: driver=%.*s object=%.*s status=%d\n",
		       (int)sizeof(fences[i].driver_name), fences[i].driver_name,
		       (int)sizeof(fences[i].obj_name), fences[i].obj_name, fences[i].status);
		if (fences[i].status != 1)
			goto out;
	}
	result = 0;
out:
	free(fences);
	return result;
}
