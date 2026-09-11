// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/file.h>
#include <drm/drm_atomic_state_helper.h>
#include <drm/drm_auth.h>
#include <drm/drm_connector.h>
#include <drm/drm_crtc.h>
#include <drm/drm_file.h>
#include <drm/drm_ioctl.h>
#include <drm/drm_kunit_helpers.h>
#include <drm/drm_lease.h>
#include <drm/drm_modeset_lock.h>
#include <drm/drm_property.h>
#include <kunit/test.h>

#include "../drm_atomic_user_input.h"
#include "../drm_atomic_user_request.h"

static const struct file_operations test_fops = {
	.owner = THIS_MODULE,
	.release = drm_release_noglobal,
};

static const struct drm_driver test_driver = {
	.driver_features = DRIVER_MODESET | DRIVER_ATOMIC,
	.fops = &test_fops,
};

struct auth_fixture {
	struct drm_device *dev;
	struct drm_plane *plane;
	struct drm_crtc *crtc;
	struct drm_connector connector;
	struct file *root;
};

static void close_file(void *data)
{
	__fput_sync(data);
}

/* Model file authority without registering device nodes or leasing real outputs. */
static struct file *new_master_file(struct kunit *test, struct drm_device *dev,
				    struct drm_master *lessor)
{
	struct file *file = mock_drm_getfile(dev->primary, O_RDWR);
	struct drm_file *priv;
	struct drm_master *master;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, file);
	atomic_inc(&dev->open_count);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, close_file, file), 0);
	priv = file->private_data;
	master = kzalloc_obj(*master);
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

static struct auth_fixture *new_fixture(struct kunit *test)
{
	struct auth_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct device *parent = drm_kunit_helper_alloc_device(test);

	KUNIT_ASSERT_NOT_NULL(test, f);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	f->dev = __drm_kunit_helper_alloc_drm_device_with_driver(test, parent,
							   sizeof(*f->dev), 0, &test_driver);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->dev);
	f->plane = drm_kunit_helper_create_primary_plane(test, f->dev, NULL, NULL, NULL, 0, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->plane);
	f->crtc = drm_kunit_helper_create_crtc(test, f->dev, f->plane, NULL, NULL, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->crtc);
	f->root = new_master_file(test, f->dev, NULL);
	return f;
}

static int lock_display(struct drm_device *dev, struct drm_modeset_acquire_ctx *ctx)
{
	int ret;

	drm_modeset_acquire_init(ctx, 0);
	for (;;) {
		ret = drm_modeset_lock_all_ctx(dev, ctx);
		if (ret != -EDEADLK)
			return ret;
		ret = drm_modeset_backoff(ctx);
		if (ret)
			return ret;
	}
}

static void unlock_display(struct drm_modeset_acquire_ctx *ctx)
{
	drm_modeset_drop_locks(ctx);
	drm_modeset_acquire_fini(ctx);
}

static void free_request(void *data)
{
	drm_atomic_free_user_request(data);
}

static struct drm_atomic_user_request *new_request(struct kunit *test, struct auth_fixture *f,
		struct drm_mode_object *object, struct drm_property *property, u64 value)
{
	u32 object_id = object->id, property_id = property ? property->base.id : 0;
	u32 count = property ? 1 : 0;
	struct drm_atomic_user_input input = {
		.object_count = 1, .property_count = count, .objects = &object_id,
		.counts = &count, .properties = &property_id, .values = &value,
	};
	struct drm_modeset_acquire_ctx ctx;
	struct drm_atomic_user_request *request;
	int ret = lock_display(f->dev, &ctx);

	request = ret ? ERR_PTR(ret) :
		drm_atomic_resolve_user_request(f->dev, f->root->private_data, &input);
	unlock_display(&ctx);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, request);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, free_request, request), 0);
	return request;
}

static int validate_request(struct auth_fixture *f, const struct drm_atomic_user_request *request,
			    struct file *file)
{
	struct drm_modeset_acquire_ctx ctx;
	int ret = lock_display(f->dev, &ctx);

	if (!ret)
		ret = drm_atomic_validate_user_request(request, file ? file->private_data : NULL);
	unlock_display(&ctx);
	return ret;
}

static void master_loss_rejects_retained_request(struct kunit *test)
{
	struct auth_fixture *f = new_fixture(test);
	struct drm_atomic_user_request *request = new_request(test, f, &f->crtc->base,
							      f->dev->mode_config.prop_active, 0);

	KUNIT_ASSERT_EQ(test, validate_request(f, request, f->root), 0);
	KUNIT_ASSERT_EQ(test, drm_ioctl(f->root, DRM_IOCTL_DROP_MASTER, 0), 0);
	KUNIT_EXPECT_EQ(test, validate_request(f, request, f->root), -EACCES);
	KUNIT_EXPECT_EQ(test, validate_request(f, request, NULL), -EACCES);
}

static struct file *new_lessee(struct kunit *test, struct auth_fixture *f)
{
	struct drm_file *root = f->root->private_data;

	return new_master_file(test, f->dev, root->master);
}

static void allow_object(struct kunit *test, struct file *file, struct drm_mode_object *object)
{
	struct drm_file *priv = file->private_data;
	int ret;

	mutex_lock(&priv->minor->dev->mode_config.idr_mutex);
	ret = idr_alloc(&priv->master->leases, object, object->id, object->id + 1, GFP_KERNEL);
	mutex_unlock(&priv->minor->dev->mode_config.idr_mutex);
	KUNIT_ASSERT_EQ(test, ret, object->id);
}

static void empty_group_still_requires_a_lease(struct kunit *test)
{
	struct auth_fixture *f = new_fixture(test);
	struct drm_atomic_user_request *request = new_request(test, f, &f->crtc->base, NULL, 0);
	struct file *child = new_lessee(test, f);

	allow_object(test, child, &f->crtc->base);
	KUNIT_ASSERT_EQ(test, validate_request(f, request, child), 0);
	drm_lease_revoke(((struct drm_file *)child->private_data)->master);
	KUNIT_EXPECT_EQ(test, validate_request(f, request, child), -EACCES);
}

static void referenced_controller_requires_its_own_lease(struct kunit *test)
{
	struct auth_fixture *f = new_fixture(test);
	struct drm_atomic_user_request *request = new_request(test, f, &f->plane->base,
						f->dev->mode_config.prop_crtc_id, f->crtc->base.id);
	struct file *child = new_lessee(test, f);

	allow_object(test, child, &f->plane->base);
	KUNIT_EXPECT_EQ(test, validate_request(f, request, child), -EACCES);
	allow_object(test, child, &f->crtc->base);
	KUNIT_EXPECT_EQ(test, validate_request(f, request, child), 0);
}

static const struct drm_connector_funcs connector_funcs = {
	.reset = drm_atomic_helper_connector_reset,
	.atomic_duplicate_state = drm_atomic_helper_connector_duplicate_state,
	.atomic_destroy_state = drm_atomic_helper_connector_destroy_state,
};

static void unregistered_connector_is_not_restored_by_retention(struct kunit *test)
{
	struct auth_fixture *f = new_fixture(test);
	struct drm_atomic_user_request *request;
	void *old;

	KUNIT_ASSERT_EQ(test, drmm_connector_init(f->dev, &f->connector, &connector_funcs,
						DRM_MODE_CONNECTOR_VIRTUAL, NULL), 0);
	/* Publish only in the fixture's lookup table, without registering sysfs nodes. */
	mutex_lock(&f->dev->mode_config.idr_mutex);
	old = idr_replace(&f->dev->mode_config.object_idr, &f->connector.base,
			  f->connector.base.id);
	mutex_unlock(&f->dev->mode_config.idr_mutex);
	KUNIT_ASSERT_NULL(test, old);
	request = new_request(test, f, &f->connector.base, NULL, 0);
	KUNIT_ASSERT_EQ(test, validate_request(f, request, f->root), 0);
	WRITE_ONCE(f->connector.registration_state, DRM_CONNECTOR_UNREGISTERED);
	KUNIT_EXPECT_EQ(test, validate_request(f, request, f->root), -ENOENT);
}

static struct kunit_case cases[] = {
	KUNIT_CASE(master_loss_rejects_retained_request),
	KUNIT_CASE(empty_group_still_requires_a_lease),
	KUNIT_CASE(referenced_controller_requires_its_own_lease),
	KUNIT_CASE(unregistered_connector_is_not_restored_by_retention),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_user_request_auth",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_IMPORT_NS("EXPORTED_FOR_KUNIT_TESTING");
MODULE_LICENSE("GPL");
