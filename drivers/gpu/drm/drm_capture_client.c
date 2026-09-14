// SPDX-License-Identifier: GPL-2.0-only
/* Capture client lifetime, separate from the authority's revocation owner. */

#include <linux/anon_inodes.h>
#include <linux/err.h>
#include <linux/fs.h>
#include <linux/module.h>
#include <linux/poll.h>
#include <linux/slab.h>

#include <drm/drm_capture_authority.h>
#include <drm/drm_capture_file.h>

struct drm_capture_client {
	struct drm_capture_authority *authority;
	const struct drm_capture_client_owner_ops *ops;
	void *data;
};

static int capture_client_release(struct inode *inode, struct file *file)
{
	struct drm_capture_client *client = file->private_data;
	struct module *owner = client->ops->owner;

	client->ops->release(client->data);
	drm_capture_authority_put(client->authority);
	kfree(client);
	module_put(owner);
	return 0;
}

static __poll_t capture_client_poll(struct file *file, poll_table *wait)
{
	struct drm_capture_client *client = file->private_data;

	poll_wait(file, drm_capture_authority_waitqueue(client->authority), wait);
	return drm_capture_authority_cleanup_done(client->authority) ? EPOLLHUP : 0;
}

static const struct file_operations capture_client_fops = {
	.owner = THIS_MODULE,
	.release = capture_client_release,
	.poll = capture_client_poll,
};

struct file *drm_capture_client_file_create(
	struct drm_capture_authority *authority,
	const struct drm_capture_client_owner_ops *ops, void *data)
{
	struct drm_capture_client *client;
	struct file *file;

	if (!ops || !ops->release)
		return ERR_PTR(-EINVAL);
	client = kzalloc_obj(*client);
	if (!client)
		return ERR_PTR(-ENOMEM);
	if (!try_module_get(ops->owner)) {
		kfree(client);
		return ERR_PTR(-ENODEV);
	}
	client->authority = drm_capture_authority_get(authority);
	client->ops = ops;
	client->data = data;
	file = anon_inode_getfile("drm-capture", &capture_client_fops, client, O_RDONLY);
	if (IS_ERR(file)) {
		drm_capture_authority_put(client->authority);
		module_put(ops->owner);
		kfree(client);
	}
	return file;
}
EXPORT_SYMBOL_GPL(drm_capture_client_file_create);
