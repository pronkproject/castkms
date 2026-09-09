// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/anon_inodes.h>
#include <linux/err.h>
#include <linux/fs.h>
#include <linux/module.h>
#include <linux/poll.h>
#include <drm/drm_atomic_prepare_file.h>
#include <drm/drm_atomic_prepare_ticket.h>

static int ticket_release(struct inode *inode, struct file *file)
{
	struct drm_prepare_ticket *ticket = file->private_data;

	drm_prepare_ticket_cancel(ticket);
	drm_prepare_ticket_put(ticket);
	return 0;
}

static __poll_t ticket_poll(struct file *file, poll_table *wait)
{
	struct drm_prepare_ticket *ticket = file->private_data;

	poll_wait(file, drm_prepare_ticket_waitqueue(ticket), wait);
	switch (drm_prepare_ticket_ready(ticket)) {
	case 0:
		return EPOLLIN | EPOLLRDNORM;
	case -EAGAIN:
		return 0;
	case -EALREADY:
		return EPOLLHUP;
	default:
		return EPOLLERR | EPOLLHUP;
	}
}

static const struct file_operations ticket_fops = {
	.owner = THIS_MODULE,
	.release = ticket_release,
	.poll = ticket_poll,
};

struct file *drm_prepare_ticket_file_create(struct drm_prepare_ticket *ticket)
{
	struct file *file;

	file = anon_inode_getfile("drm-preparation", &ticket_fops, NULL, O_RDONLY);
	if (IS_ERR(file))
		return file;
	file->private_data = drm_prepare_ticket_get(ticket);
	return file;
}
EXPORT_SYMBOL_GPL(drm_prepare_ticket_file_create);

struct drm_prepare_ticket *drm_prepare_ticket_file_get_ticket(struct file *file)
{
	if (file->f_op != &ticket_fops)
		return ERR_PTR(-EINVAL);
	return drm_prepare_ticket_get(file->private_data);
}
EXPORT_SYMBOL_GPL(drm_prepare_ticket_file_get_ticket);
