// SPDX-License-Identifier: GPL-2.0-only
/* Anonymous revocation endpoint; authority policy lives outside the file. */

#include <linux/anon_inodes.h>
#include <linux/err.h>
#include <linux/fs.h>
#include <linux/module.h>
#include <linux/poll.h>

#include <drm/drm_capture_authority.h>
#include <drm/drm_capture_file.h>

static int drm_capture_control_release(struct inode *inode, struct file *file)
{
	struct drm_capture_authority *authority = file->private_data;

	drm_capture_authority_revoke(authority);
	drm_capture_authority_put(authority);
	return 0;
}

static __poll_t drm_capture_control_poll(struct file *file, poll_table *wait)
{
	struct drm_capture_authority *authority = file->private_data;

	poll_wait(file, drm_capture_authority_waitqueue(authority), wait);
	return drm_capture_authority_cleanup_done(authority) ? EPOLLHUP : 0;
}

static const struct file_operations drm_capture_control_fops = {
	.owner = THIS_MODULE,
	.release = drm_capture_control_release,
	.poll = drm_capture_control_poll,
};

struct file *drm_capture_control_file_create(struct drm_capture_authority *authority)
{
	struct file *file;

	file = anon_inode_getfile("drm-capture-control", &drm_capture_control_fops, NULL, O_RDONLY);
	if (IS_ERR(file))
		return file;
	/* No descriptor exposes the file before its authority reference is owned. */
	file->private_data = drm_capture_authority_get(authority);
	return file;
}
EXPORT_SYMBOL_GPL(drm_capture_control_file_create);
