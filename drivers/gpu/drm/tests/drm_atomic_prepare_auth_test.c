// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/file.h>
#include <linux/module.h>
#include <drm/drm_atomic_prepare_auth.h>
#include <drm/drm_atomic_prepare_owner.h>
#include <drm/drm_atomic_prepare_ticket.h>
#include <drm/drm_auth.h>
#include <drm/drm_device.h>
#include <drm/drm_file.h>
#include <drm/drm_ioctl.h>
#include <drm/drm_kunit_helpers.h>
#include <drm/drm_lease.h>
#include <kunit/test.h>

static const struct file_operations test_fops = {
	.owner = THIS_MODULE,
	.release = drm_release_noglobal,
};

static const struct drm_driver test_driver = {
	.driver_features = DRIVER_MODESET,
	.fops = &test_fops,
};

static void close_file(void *data)
{
	__fput_sync(data);
}

static void put_owner(void *data)
{
	drm_prepare_owner_put(data);
}

static void put_ticket(void *data)
{
	drm_prepare_ticket_put(data);
}

static struct drm_device *new_device(struct kunit *test)
{
	struct device *parent = drm_kunit_helper_alloc_device(test);
	struct drm_device *dev;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	dev = __drm_kunit_helper_alloc_drm_device_with_driver(test, parent,
							   sizeof(*dev), 0, &test_driver);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, dev);
	return dev;
}

static struct file *new_file(struct kunit *test, struct drm_device *dev)
{
	struct file *file = mock_drm_getfile(dev->primary, O_RDWR);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, file);
	atomic_inc(&dev->open_count);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, close_file, file), 0);
	return file;
}

/* Construct authority without registering device nodes or leasing real outputs. */
static struct file *new_master_file(struct kunit *test, struct drm_device *dev,
				    struct drm_master *lessor)
{
	struct file *file = new_file(test, dev);
	struct drm_file *priv = file->private_data;
	struct drm_master *master = kzalloc_obj(*master);

	KUNIT_ASSERT_NOT_NULL(test, master);
	kref_init(&master->refcount);
	master->dev = dev;
	idr_init_base(&master->magic_map, 1);
	idr_init(&master->leases);
	idr_init_base(&master->lessee_idr, 1);
	INIT_LIST_HEAD(&master->lessees);
	INIT_LIST_HEAD(&master->lessee_list);
	priv->master = master;
	priv->is_master = true;
	priv->was_master = true;
	if (lessor) {
		guard(mutex)(&dev->mode_config.idr_mutex);
		master->lessor = drm_master_get(lessor);
		list_add_tail(&master->lessee_list, &lessor->lessees);
	} else {
		guard(mutex)(&dev->master_mutex);
		dev->master = drm_master_get(master);
	}
	return file;
}

static struct drm_prepare_owner *owner_for(struct kunit *test, struct file *file)
{
	struct drm_prepare_owner *owner = drm_file_prepare_owner(file->private_data);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, owner);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_owner, owner), 0);
	return owner;
}

static struct drm_prepare_ticket *ticket_for(struct kunit *test,
					    struct drm_prepare_owner *owner)
{
	struct drm_prepare_ticket *ticket = drm_prepare_ticket_create_owned(owner, NULL, 0);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, ticket);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_ticket, ticket), 0);
	return ticket;
}

static void associated_file_cannot_borrow_issuer(struct kunit *test)
{
	struct drm_device *dev = new_device(test);
	struct file *master_file = new_master_file(test, dev, NULL);
	struct drm_file *master_priv = master_file->private_data;
	struct file *other = new_file(test, dev);
	struct drm_file *other_priv = other->private_data;
	struct drm_prepare_owner *owner = owner_for(test, master_file);
	struct drm_prepare_ticket *ticket = ticket_for(test, owner);

	other_priv->master = drm_master_get(master_priv->master);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_file_prepare_owner(other_priv)), -EACCES);
	KUNIT_EXPECT_PTR_EQ(test, owner_for(test, master_file), owner);
	kunit_release_action(test, close_file, other);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_status(ticket), DRM_PREPARE_TICKET_READY);
}

static void dropping_master_cancels_before_reacquisition(struct kunit *test)
{
	struct drm_device *dev = new_device(test);
	struct file *file = new_master_file(test, dev, NULL);
	struct drm_prepare_owner *owner = owner_for(test, file);
	struct drm_prepare_ticket *ticket = ticket_for(test, owner);

	KUNIT_ASSERT_EQ(test, drm_ioctl(file, DRM_IOCTL_DROP_MASTER, 0), 0);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_status(ticket), DRM_PREPARE_TICKET_CANCELED);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_file_prepare_owner(file->private_data)), -EACCES);
	KUNIT_ASSERT_EQ(test, drm_ioctl(file, DRM_IOCTL_SET_MASTER, 0), 0);
	KUNIT_EXPECT_PTR_NE(test, owner_for(test, file), owner);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_ticket_create_owned(owner, NULL, 0)), -ECANCELED);
}

static void final_file_close_cancels_retained_tickets(struct kunit *test)
{
	struct drm_device *dev = new_device(test);
	struct file *file = new_master_file(test, dev, NULL);
	struct drm_prepare_owner *owner = owner_for(test, file);
	struct drm_prepare_ticket *ticket = ticket_for(test, owner);

	get_file(file);
	__fput_sync(file);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_status(ticket), DRM_PREPARE_TICKET_READY);
	kunit_release_action(test, close_file, file);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_status(ticket), DRM_PREPARE_TICKET_CANCELED);
	KUNIT_EXPECT_PTR_EQ(test, dev->master, NULL);
}

static void lease_revocation_cancels_descendants_only(struct kunit *test)
{
	struct drm_device *dev = new_device(test);
	struct file *root = new_master_file(test, dev, NULL);
	struct drm_file *root_priv = root->private_data;
	struct file *child = new_master_file(test, dev, root_priv->master);
	struct drm_file *child_priv = child->private_data;
	struct file *grandchild = new_master_file(test, dev, child_priv->master);
	struct file *sibling = new_master_file(test, dev, root_priv->master);
	struct drm_prepare_ticket *root_ticket = ticket_for(test, owner_for(test, root));
	struct drm_prepare_ticket *child_ticket = ticket_for(test, owner_for(test, child));
	struct drm_prepare_ticket *grandchild_ticket = ticket_for(test, owner_for(test, grandchild));
	struct drm_prepare_ticket *sibling_ticket = ticket_for(test, owner_for(test, sibling));

	drm_lease_revoke(child_priv->master);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_status(child_ticket), DRM_PREPARE_TICKET_CANCELED);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_status(grandchild_ticket), DRM_PREPARE_TICKET_CANCELED);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_status(root_ticket), DRM_PREPARE_TICKET_READY);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_status(sibling_ticket), DRM_PREPARE_TICKET_READY);
	KUNIT_ASSERT_EQ(test, drm_ioctl(root, DRM_IOCTL_DROP_MASTER, 0), 0);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_status(root_ticket), DRM_PREPARE_TICKET_CANCELED);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_status(sibling_ticket), DRM_PREPARE_TICKET_CANCELED);
}

static struct kunit_case cases[] = {
	KUNIT_CASE(associated_file_cannot_borrow_issuer),
	KUNIT_CASE(dropping_master_cancels_before_reacquisition),
	KUNIT_CASE(final_file_close_cancels_retained_tickets),
	KUNIT_CASE(lease_revocation_cancels_descendants_only),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_prepare_auth",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_LICENSE("GPL");
