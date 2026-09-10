// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/anon_inodes.h>
#include <linux/err.h>
#include <linux/fs.h>
#include <linux/module.h>
#include <linux/poll.h>
#include <linux/uaccess.h>
#include <drm/drm_atomic_prepare_file.h>
#include <drm/drm_atomic_prepare_ticket.h>
#include <uapi/drm/drm_prepare.h>

static u32 ticket_query_status(struct drm_prepare_ticket *ticket)
{
	switch (drm_prepare_ticket_status(ticket)) {
	case DRM_PREPARE_TICKET_PENDING:
		return DRM_PREPARE_PENDING;
	case DRM_PREPARE_TICKET_READY:
		return DRM_PREPARE_READY;
	case DRM_PREPARE_TICKET_CONSUMED:
		return DRM_PREPARE_CONSUMED;
	case DRM_PREPARE_TICKET_CANCELED:
		return DRM_PREPARE_CANCELED;
	case DRM_PREPARE_TICKET_FAILED:
		return DRM_PREPARE_FAILED;
	}
	return DRM_PREPARE_FAILED;
}

static long ticket_ioctl(struct file *file, unsigned int cmd, unsigned long arg)
{
	struct drm_prepare_query query = {};

	if (cmd != DRM_IOCTL_PREPARE_QUERY)
		return -ENOTTY;
	query.status = ticket_query_status(file->private_data);
	if (copy_to_user((void __user *)arg, &query, sizeof(query)))
		return -EFAULT;
	return 0;
}

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
	switch (drm_prepare_ticket_status(ticket)) {
	case DRM_PREPARE_TICKET_READY:
		return EPOLLIN | EPOLLRDNORM;
	case DRM_PREPARE_TICKET_PENDING:
		return 0;
	case DRM_PREPARE_TICKET_CONSUMED:
		return EPOLLHUP;
	case DRM_PREPARE_TICKET_CANCELED:
	case DRM_PREPARE_TICKET_FAILED:
		return EPOLLERR | EPOLLHUP;
	}
	return EPOLLERR | EPOLLHUP;
}

static const struct file_operations ticket_fops = {
	.owner = THIS_MODULE,
	.release = ticket_release,
	.poll = ticket_poll,
	.unlocked_ioctl = ticket_ioctl,
	.compat_ioctl = compat_ptr_ioctl,
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

int drm_atomic_commit_prepare_file(struct drm_atomic_commit *state,
				  struct file *file,
				  struct drm_prepare_owner *owner,
				  drm_atomic_prepare_observe_fn observe)
{
	struct drm_prepare_ticket *ticket;
	int ret;

	ticket = drm_prepare_ticket_file_get_ticket(file);
	if (IS_ERR(ticket))
		return PTR_ERR(ticket);
	ret = drm_atomic_commit_prepare_owned(state, ticket, owner, observe);
	drm_prepare_ticket_put(ticket);
	return ret;
}
EXPORT_SYMBOL_GPL(drm_atomic_commit_prepare_file);
