// SPDX-License-Identifier: GPL-2.0-only
/* Anonymous revocation endpoint; authority policy lives outside the file. */

#include <linux/anon_inodes.h>
#include <linux/err.h>
#include <linux/fs.h>
#include <linux/module.h>
#include <linux/poll.h>
#include <linux/slab.h>

#include <drm/drm_capture_authority.h>
#include <drm/drm_capture_file.h>

struct drm_capture_control {
	struct drm_capture_authority *authority;
	const struct drm_capture_control_owner_ops *ops;
	void *data;
};

static int drm_capture_control_release(struct inode *inode, struct file *file)
{
	struct drm_capture_control *control = file->private_data;
	struct module *owner = control->ops ? control->ops->owner : NULL;

	drm_capture_authority_revoke(control->authority);
	if (control->ops)
		control->ops->release(control->data);
	drm_capture_authority_put(control->authority);
	kfree(control);
	module_put(owner);
	return 0;
}

static __poll_t drm_capture_control_poll(struct file *file, poll_table *wait)
{
	struct drm_capture_control *control = file->private_data;

	poll_wait(file, drm_capture_authority_waitqueue(control->authority), wait);
	return drm_capture_authority_cleanup_done(control->authority) ? EPOLLHUP : 0;
}

static const struct file_operations drm_capture_control_fops = {
	.owner = THIS_MODULE,
	.release = drm_capture_control_release,
	.poll = drm_capture_control_poll,
};

static struct file *control_file_create(struct drm_capture_authority *authority,
				       const struct drm_capture_control_owner_ops *ops,
				       void *data)
{
	struct drm_capture_control *control;
	struct file *file;

	control = kzalloc(sizeof(*control), GFP_KERNEL);
	if (!control)
		return ERR_PTR(-ENOMEM);
	if (ops && !try_module_get(ops->owner)) {
		kfree(control);
		return ERR_PTR(-ENODEV);
	}
	control->authority = drm_capture_authority_get(authority);
	control->ops = ops;
	control->data = data;
	file = anon_inode_getfile("drm-capture-control", &drm_capture_control_fops,
				  control, O_RDONLY);
	if (IS_ERR(file)) {
		drm_capture_authority_put(control->authority);
		if (ops)
			module_put(ops->owner);
		kfree(control);
	}
	return file;
}

struct file *drm_capture_control_file_create(struct drm_capture_authority *authority)
{
	return control_file_create(authority, NULL, NULL);
}
EXPORT_SYMBOL_GPL(drm_capture_control_file_create);

struct file *drm_capture_control_file_create_owned(
	struct drm_capture_authority *authority,
	const struct drm_capture_control_owner_ops *ops, void *data)
{
	if (!ops || !ops->release)
		return ERR_PTR(-EINVAL);
	return control_file_create(authority, ops, data);
}
EXPORT_SYMBOL_GPL(drm_capture_control_file_create_owned);
