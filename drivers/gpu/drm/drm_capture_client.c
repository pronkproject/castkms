// SPDX-License-Identifier: GPL-2.0-only
/* Capture client lifetime, separate from the authority's revocation owner. */

#include <linux/anon_inodes.h>
#include <linux/err.h>
#include <linux/fs.h>
#include <linux/module.h>
#include <linux/mutex.h>
#include <linux/poll.h>
#include <linux/slab.h>

#include <drm/drm_capture_authority.h>
#include <drm/drm_capture_destination.h>
#include <drm/drm_capture_file.h>
#include <drm/drm_capture_readiness.h>
#include <drm/drm_fourcc.h>

#include "drm_capture_file_internal.h"
#include "drm_capture_uapi.h"

struct drm_capture_client {
	/* Serializes mutable provider callbacks, never held by authority revocation. */
	struct mutex lock;
	struct drm_capture_authority *authority;
	struct drm_capture_readiness *readiness;
	const struct drm_capture_client_owner_ops *ops;
	void *data;
};

static int capture_client_release(struct inode *inode, struct file *file)
{
	struct drm_capture_client *client = file->private_data;
	struct module *owner = client->ops->owner;

	client->ops->release(client->data);
	drm_capture_readiness_put(client->readiness);
	mutex_destroy(&client->lock);
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
	.unlocked_ioctl = drm_capture_client_ioctl,
	.compat_ioctl = compat_ptr_ioctl,
};

static struct drm_capture_client *capture_client_from_file(struct file *file)
{
	if (!file || file->f_op != &capture_client_fops)
		return NULL;
	return file->private_data;
}

struct drm_capture_readiness *drm_capture_client_get_readiness(struct file *file)
{
	struct drm_capture_client *client = capture_client_from_file(file);

	if (!client)
		return ERR_PTR(-EINVAL);
	if (!client->readiness)
		return ERR_PTR(-EOPNOTSUPP);
	return drm_capture_readiness_get(client->readiness);
}
EXPORT_SYMBOL_GPL(drm_capture_client_get_readiness);

int drm_capture_client_describe(struct file *file,
			       struct drm_capture_description *description)
{
	struct drm_capture_description result = {};
	struct drm_capture_client *client = capture_client_from_file(file);
	int ret;

	if (!client || !description)
		return -EINVAL;
	mutex_lock(&client->lock);
	if (drm_capture_authority_revoked(client->authority)) {
		ret = -EKEYREVOKED;
		goto unlock;
	}
	if (!client->ops->describe) {
		ret = -EOPNOTSUPP;
		goto unlock;
	}
	ret = client->ops->describe(client->data, &result);
	if (ret < 0)
		goto unlock;
	if (ret || !result.id || !result.width || !result.height || !result.format ||
	    !result.max_requests || result.modifier == DRM_FORMAT_MOD_INVALID) {
		ret = -EINVAL;
		goto unlock;
	}
	*description = result;
unlock:
	mutex_unlock(&client->lock);
	return ret;
}
EXPORT_SYMBOL_GPL(drm_capture_client_describe);

int drm_capture_client_open_stream(struct file *file, u64 id, u64 offer, u32 capacity)
{
	struct drm_capture_client *client = capture_client_from_file(file);
	int ret;

	if (!client || !id || !offer || !capacity)
		return -EINVAL;
	mutex_lock(&client->lock);
	if (drm_capture_authority_revoked(client->authority))
		ret = -EKEYREVOKED;
	else if (!client->ops->open_stream || !client->ops->close_stream)
		ret = -EOPNOTSUPP;
	else
		ret = client->ops->open_stream(client->data, id, offer, capacity);
	mutex_unlock(&client->lock);
	return ret > 0 ? -EINVAL : ret;
}
EXPORT_SYMBOL_GPL(drm_capture_client_open_stream);

int drm_capture_client_close_stream(struct file *file, u64 id)
{
	struct drm_capture_client *client = capture_client_from_file(file);
	int ret;

	if (!client || !id)
		return -EINVAL;
	mutex_lock(&client->lock);
	/* Cleanup is independent of permission to admit another capture operation. */
	if (!client->ops->close_stream)
		ret = -EOPNOTSUPP;
	else
		ret = client->ops->close_stream(client->data, id);
	mutex_unlock(&client->lock);
	return ret > 0 ? -EINVAL : ret;
}
EXPORT_SYMBOL_GPL(drm_capture_client_close_stream);

int drm_capture_client_register_destination(struct file *file, u64 id,
					    const struct drm_capture_destination *destination)
{
	struct drm_capture_client *client = capture_client_from_file(file);
	int ret;

	if (!client || !id)
		return -EINVAL;
	ret = drm_capture_destination_validate(destination);
	if (ret)
		return ret;
	mutex_lock(&client->lock);
	if (drm_capture_authority_revoked(client->authority))
		ret = -EKEYREVOKED;
	else if (!client->ops->register_destination || !client->ops->unregister_destination)
		ret = -EOPNOTSUPP;
	else
		ret = client->ops->register_destination(client->data, id, destination);
	mutex_unlock(&client->lock);
	return ret > 0 ? -EINVAL : ret;
}
EXPORT_SYMBOL_GPL(drm_capture_client_register_destination);

int drm_capture_client_unregister_destination(struct file *file, u64 id)
{
	struct drm_capture_client *client = capture_client_from_file(file);
	int ret;

	if (!client || !id)
		return -EINVAL;
	mutex_lock(&client->lock);
	if (!client->ops->unregister_destination)
		ret = -EOPNOTSUPP;
	else
		ret = client->ops->unregister_destination(client->data, id);
	mutex_unlock(&client->lock);
	return ret > 0 ? -EINVAL : ret;
}
EXPORT_SYMBOL_GPL(drm_capture_client_unregister_destination);

struct drm_capture_authority *drm_capture_client_authority(struct file *file)
{
	struct drm_capture_client *client = capture_client_from_file(file);

	if (!client)
		return NULL;
	return client->authority;
}

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
	mutex_init(&client->lock);
	client->ops = ops;
	client->data = data;
	if (ops->get_readiness)
		client->readiness = ops->get_readiness(data);
	file = anon_inode_getfile("drm-capture", &capture_client_fops, client, O_RDONLY);
	if (IS_ERR(file)) {
		drm_capture_readiness_put(client->readiness);
		mutex_destroy(&client->lock);
		drm_capture_authority_put(client->authority);
		module_put(ops->owner);
		kfree(client);
	}
	return file;
}
EXPORT_SYMBOL_GPL(drm_capture_client_file_create);
