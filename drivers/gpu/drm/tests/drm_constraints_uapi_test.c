// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/file.h>
#include <linux/mman.h>
#include <linux/module.h>
#include <linux/uaccess.h>
#include <drm/drm_auth.h>
#include <drm/drm_constraints.h>
#include <drm/drm_constraints_device.h>
#include <drm/drm_constraints_entry.h>
#include <drm/drm_constraints_list.h>
#include <drm/drm_constraints_output.h>
#include <drm/drm_constraints_owner.h>
#include <drm/drm_crtc.h>
#include <drm/drm_file.h>
#include <drm/drm_fourcc.h>
#include <drm/drm_ioctl.h>
#include <drm/drm_kunit_helpers.h>
#include <uapi/drm/drm_constraints.h>
#include <kunit/test.h>

#include "../drm_internal.h"

static const struct file_operations test_fops = {
	.owner = THIS_MODULE,
	.release = drm_release_noglobal,
	.unlocked_ioctl = drm_ioctl,
};

static const struct drm_driver test_driver = {
	.driver_features = DRIVER_MODESET | DRIVER_ATOMIC,
	.fops = &test_fops,
};

struct query_fixture {
	struct drm_device *dev;
	struct drm_crtc *crtc;
	struct drm_crtc *unattached;
	struct file *file;
};

static void close_file(void *data) { __fput_sync(data); }
static void stop_owner(void *data) { drm_constraints_owner_stop(data); }
static void put_description(void *data) { drm_constraints_description_put(data); }
static void put_entry(void *data) { drm_constraints_entry_put(data); }
static void put_master(void *data)
{
	struct drm_master *master = data;

	drm_master_put(&master);
}

static int check(const struct drm_atomic_commit *state, const struct drm_crtc_state *crtc,
		 const struct drm_constraints_entry *entry)
{
	return 0;
}

static const struct drm_constraints_output_ops output_ops = { .check = check };

static struct file *new_file(struct kunit *test, struct drm_device *dev)
{
	struct file *file = mock_drm_getfile(dev->primary, O_RDWR);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, file);
	atomic_inc(&dev->open_count);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, close_file, file), 0);
	return file;
}

static struct query_fixture *new_fixture(struct kunit *test)
{
	struct query_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct device *parent = drm_kunit_helper_alloc_device(test);
	const struct drm_constraints_size size = { 640, 480, 640, 480 };
	struct drm_constraints_format format = {
		.format = DRM_FORMAT_XRGB8888, .size = size,
		.flags = DRM_CONSTRAINTS_FORMAT_IMPLICIT,
	};
	struct drm_constraints_description *description;
	struct drm_constraints_entry *entry;
	struct drm_plane *plane;

	KUNIT_ASSERT_NOT_NULL(test, f);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	f->dev = __drm_kunit_helper_alloc_drm_device_with_driver(test, parent,
							      sizeof(*f->dev), 0, &test_driver);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->dev);
	KUNIT_ASSERT_EQ(test, drm_constraints_device_init(f->dev, 8), 0);
	plane = drm_kunit_helper_create_primary_plane(test, f->dev, NULL, NULL, NULL, 0, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, plane);
	format.plane_id = plane->base.id;
	f->crtc = drm_kunit_helper_create_crtc(test, f->dev, plane, NULL, NULL, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->crtc);
	plane = drm_kunit_helper_create_primary_plane(test, f->dev, NULL, NULL, NULL, 0, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, plane);
	f->unattached = drm_kunit_helper_create_crtc(test, f->dev, plane, NULL, NULL, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->unattached);
	drm_mode_config_reset(f->dev);
	description = drm_constraints_description_create(&size, &format, 1, NULL, 0);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, description);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_description, description), 0);
	entry = drm_constraints_entry_create_stateless(drm_constraints_device_domain(f->dev),
							 f->crtc->base.id, description);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, entry);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_entry, entry), 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_crtc_init(f->crtc, entry, 4, &output_ops), 0);
	f->file = new_file(test, f->dev);
	KUNIT_ASSERT_EQ(test, drm_master_open(f->file->private_data), 0);
	/* Stop recovery before the fixture's owning master closes. */
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, stop_owner, f->dev), 0);
	return f;
}

static unsigned long user_page(struct kunit *test)
{
	unsigned long address;

	if (!IS_ENABLED(CONFIG_MMU))
		kunit_skip(test, "ioctl copyout requires MMU");
	address = kunit_vm_mmap(test, NULL, 0, PAGE_SIZE, PROT_READ | PROT_WRITE,
			       MAP_PRIVATE | MAP_ANONYMOUS, 0);
	KUNIT_ASSERT_NE(test, address, 0);
	KUNIT_ASSERT_FALSE(test, IS_ERR_VALUE(address));
	return address;
}

static void query_ioctl(struct kunit *test, struct file *file, unsigned long address,
			struct drm_mode_list_constraints *query, long expected)
{
	KUNIT_ASSERT_EQ(test, copy_to_user((void __user *)address, query, sizeof(*query)), 0);
	KUNIT_ASSERT_EQ(test, file->f_op->unlocked_ioctl(file, DRM_IOCTL_MODE_LIST_CONSTRAINTS,
						      address), expected);
	KUNIT_ASSERT_EQ(test, copy_from_user(query, (void __user *)address, sizeof(*query)), 0);
}

static void ioctl_publishes_sizing_metadata_on_enospc(struct kunit *test)
{
	struct query_fixture *f = new_fixture(test);
	struct drm_mode_list_constraints query = { .crtc_id = f->crtc->base.id };
	struct drm_mode_constraints_list header;
	unsigned long address = user_page(test);
	u64 generation;
	u32 required;

	KUNIT_EXPECT_FALSE(test, ((struct drm_file *)f->file->private_data)->atomic);
	query_ioctl(test, f->file, address, &query, 0);
	generation = query.generation;
	required = query.size;
	KUNIT_ASSERT_GT(test, required, (u32)sizeof(header));
	query.data = address + 128;
	query.size = 1;
	query_ioctl(test, f->file, address, &query, -ENOSPC);
	KUNIT_EXPECT_EQ(test, query.size, required);
	KUNIT_EXPECT_EQ(test, query.generation, generation);
	query_ioctl(test, f->file, address, &query, 0);
	KUNIT_ASSERT_EQ(test, copy_from_user(&header, u64_to_user_ptr(query.data),
					   sizeof(header)), 0);
	KUNIT_EXPECT_EQ(test, header.generation, generation);
	KUNIT_EXPECT_EQ(test, header.length, required);
	KUNIT_EXPECT_EQ(test, header.selected_id,
			drm_constraints_entry_id(f->crtc->state->constraints));
}

static void ioctl_checks_master_and_output_scope(struct kunit *test)
{
	struct query_fixture *f = new_fixture(test);
	struct file *other = new_file(test, f->dev);
	struct drm_mode_list_constraints query = { .crtc_id = f->crtc->base.id };
	unsigned long address = user_page(test);

	query_ioctl(test, other, address, &query, -EACCES);
	query.crtc_id = U32_MAX;
	query_ioctl(test, f->file, address, &query, -ENOENT);
	query.crtc_id = f->unattached->base.id;
	query_ioctl(test, f->file, address, &query, -EOPNOTSUPP);
	query.crtc_id = f->crtc->base.id;
	drm_constraints_list_close(drm_constraints_crtc_list(f->crtc));
	query_ioctl(test, f->file, address, &query, -ESTALE);
}

static void ioctl_respects_lease_visibility(struct kunit *test)
{
	struct query_fixture *f = new_fixture(test);
	struct drm_master *root = ((struct drm_file *)f->file->private_data)->master;
	struct drm_master *lease = drm_master_create(f->dev);
	struct drm_mode_list_constraints query = { .crtc_id = f->crtc->base.id };
	unsigned long address = user_page(test);
	struct drm_file *priv;
	struct file *file;
	int ret;

	KUNIT_ASSERT_NOT_NULL(test, lease);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_master, lease), 0);
	mutex_lock(&f->dev->mode_config.idr_mutex);
	lease->lessor = drm_master_get(root);
	list_add_tail(&lease->lessee_list, &root->lessees);
	mutex_unlock(&f->dev->mode_config.idr_mutex);
	file = new_file(test, f->dev);
	priv = file->private_data;
	if (priv->master)
		drm_master_put(&priv->master);
	priv->master = drm_master_get(lease);
	priv->is_master = true;
	query_ioctl(test, file, address, &query, -ENOENT);
	mutex_lock(&f->dev->mode_config.idr_mutex);
	ret = idr_alloc(&lease->leases, f->crtc, query.crtc_id, query.crtc_id + 1, GFP_KERNEL);
	mutex_unlock(&f->dev->mode_config.idr_mutex);
	KUNIT_ASSERT_EQ(test, ret, query.crtc_id);
	query_ioctl(test, file, address, &query, 0);
	mutex_lock(&f->dev->mode_config.idr_mutex);
	idr_remove(&lease->leases, query.crtc_id);
	mutex_unlock(&f->dev->mode_config.idr_mutex);
	query.data = 0;
	query.size = 0;
	query_ioctl(test, file, address, &query, -ENOENT);
}

static void ioctl_encoding_requires_modesetting_master(struct kunit *test)
{
	unsigned int flags;

	KUNIT_ASSERT_TRUE(test, drm_ioctl_flags(DRM_IOCTL_NR(DRM_IOCTL_MODE_LIST_CONSTRAINTS),
					      &flags));
	KUNIT_EXPECT_EQ(test, flags, DRM_MASTER);
	KUNIT_EXPECT_EQ(test, _IOC_SIZE(DRM_IOCTL_MODE_LIST_CONSTRAINTS), 48);
	KUNIT_EXPECT_EQ(test, _IOC_DIR(DRM_IOCTL_MODE_LIST_CONSTRAINTS), _IOC_READ | _IOC_WRITE);
}

static struct kunit_case constraints_uapi_cases[] = {
	KUNIT_CASE(ioctl_publishes_sizing_metadata_on_enospc),
	KUNIT_CASE(ioctl_checks_master_and_output_scope),
	KUNIT_CASE(ioctl_respects_lease_visibility),
	KUNIT_CASE(ioctl_encoding_requires_modesetting_master),
	{}
};

static struct kunit_suite constraints_uapi_suite = {
	.name = "drm_constraints_uapi",
	.test_cases = constraints_uapi_cases,
};
kunit_test_suite(constraints_uapi_suite);

MODULE_LICENSE("GPL and additional rights");
