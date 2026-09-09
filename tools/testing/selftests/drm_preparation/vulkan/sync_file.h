/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef DRM_PREPARATION_VULKAN_SYNC_FILE_H
#define DRM_PREPARATION_VULKAN_SYNC_FILE_H

/* Borrow fd, wait up to timeout_ms, and require successful native completion.
 * Vulkan's -1 sentinel represents work that has already completed.
 * A failure or timeout does not cancel work or authorize resource destruction.
 */
int gpu_sync_file_check(int fd, int timeout_ms);

#endif
