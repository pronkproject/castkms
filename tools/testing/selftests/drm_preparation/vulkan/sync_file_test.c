// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <fcntl.h>
#include <stdio.h>
#include <unistd.h>

#include "sync_file.h"

int main(void)
{
	int pipe_fds[2], null_fd, failed = 0;
	char byte = 1;

	if (pipe(pipe_fds))
		return 1;
	null_fd = open("/dev/null", O_RDONLY | O_CLOEXEC);
	if (null_fd < 0) {
		close(pipe_fds[0]);
		close(pipe_fds[1]);
		return 1;
	}
	failed |= gpu_sync_file_check(-1, 0) != 0;
	failed |= gpu_sync_file_check(-2, 0) == 0;
	failed |= gpu_sync_file_check(pipe_fds[0], -1) == 0;
	failed |= gpu_sync_file_check(pipe_fds[0], 0) == 0;
	failed |= gpu_sync_file_check(null_fd, 0) == 0;
	if (write(pipe_fds[1], &byte, sizeof(byte)) != sizeof(byte))
		failed = 1;
	else
		failed |= gpu_sync_file_check(pipe_fds[0], 0) == 0;
	close(pipe_fds[0]);
	failed |= gpu_sync_file_check(pipe_fds[0], 0) == 0;
	close(pipe_fds[1]);
	close(null_fd);
	if (!failed)
		puts("PASS: sync-file checks reject invalid, unreadable and non-fence descriptors");
	return failed ? 1 : 0;
}
