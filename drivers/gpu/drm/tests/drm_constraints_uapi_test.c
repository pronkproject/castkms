// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/file.h>
#include <linux/mman.h>
#include <linux/module.h>
#include <linux/uaccess.h>
#include <drm/drm_atomic.h>
#include <drm/drm_atomic_helper.h>
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
#include "../drm_constraints_events.h"
#include "../drm_atomic_user_input.h"
#include "../drm_atomic_user_request.h"

static const struct file_operations test_fops = {
	.owner = THIS_MODULE,
	.release = drm_release_noglobal,
	.unlocked_ioctl = drm_ioctl,
	.read = drm_read,
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
	struct file *file = mock_drm_getfile(dev->primary, O_RDWR | O_NONBLOCK);

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
		.storage_flags = DRM_CONSTRAINTS_FORMAT_STORAGE_NATIVE,
		.pitch_alignment = 1, .offset_alignment = 1, .max_pitch = U32_MAX,
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

static void client_cap(struct kunit *test, struct file *file, unsigned long address,
		       u64 capability, u64 value, long expected)
{
	struct drm_set_client_cap request = { .capability = capability, .value = value };

	KUNIT_ASSERT_EQ(test, copy_to_user((void __user *)address, &request, sizeof(request)), 0);
	KUNIT_EXPECT_EQ(test, drm_ioctl(file, DRM_IOCTL_SET_CLIENT_CAP, address), expected);
}

static void subscription_requires_atomic_support(struct kunit *test)
{
	struct query_fixture *f = new_fixture(test);
	struct drm_file *priv = f->file->private_data;
	unsigned long address = user_page(test);

	client_cap(test, f->file, address, DRM_CLIENT_CAP_KMS_CONSTRAINTS, 2, -EINVAL);
	client_cap(test, f->file, address, DRM_CLIENT_CAP_KMS_CONSTRAINTS, 1, -EOPNOTSUPP);
	KUNIT_EXPECT_FALSE(test, priv->kms_constraints);
	KUNIT_EXPECT_PTR_EQ(test, priv->constraints_events, NULL);
	client_cap(test, f->file, address, DRM_CLIENT_CAP_ATOMIC, 1, 0);
	client_cap(test, f->file, address, DRM_CLIENT_CAP_KMS_CONSTRAINTS, 1, 0);
	KUNIT_EXPECT_TRUE(test, priv->kms_constraints);
	KUNIT_ASSERT_NOT_NULL(test, priv->constraints_events);
	client_cap(test, f->file, address, DRM_CLIENT_CAP_ATOMIC, 0, -EBUSY);
	KUNIT_EXPECT_TRUE(test, priv->atomic);
	client_cap(test, f->file, address, DRM_CLIENT_CAP_KMS_CONSTRAINTS, 0, 0);
	KUNIT_EXPECT_FALSE(test, priv->kms_constraints);
	client_cap(test, f->file, address, DRM_CLIENT_CAP_ATOMIC, 0, 0);
	KUNIT_EXPECT_FALSE(test, priv->atomic);
}

static void check_unavailable_subscription(struct kunit *test, bool constraints_enabled)
{
	struct device *parent = drm_kunit_helper_alloc_device(test);
	struct drm_device *dev;
	struct drm_file *priv;
	struct file *file;
	unsigned long address = user_page(test);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	dev = __drm_kunit_helper_alloc_drm_device_with_driver(test, parent,
							    sizeof(*dev), 0, &test_driver);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, dev);
	if (constraints_enabled)
		KUNIT_ASSERT_EQ(test, drm_constraints_device_init(dev, 8), 0);
	file = new_file(test, dev);
	priv = file->private_data;
	client_cap(test, file, address, DRM_CLIENT_CAP_ATOMIC, 1, 0);
	client_cap(test, file, address, DRM_CLIENT_CAP_KMS_CONSTRAINTS, 1, -EOPNOTSUPP);
	KUNIT_EXPECT_PTR_EQ(test, priv->constraints_events, NULL);
	KUNIT_EXPECT_FALSE(test, priv->kms_constraints);
}

static void unsupported_devices_do_not_create_subscriptions(struct kunit *test)
{
	check_unavailable_subscription(test, false);
}

static void outputless_devices_do_not_publish_opt_in(struct kunit *test)
{
	check_unavailable_subscription(test, true);
}

static u64 change_suggestion(struct kunit *test, struct query_fixture *f, bool selected)
{
	struct drm_constraints_list *list = drm_constraints_crtc_list(f->crtc);
	u64 id = drm_constraints_entry_id(f->crtc->state->constraints);
	u64 generation;

	KUNIT_ASSERT_EQ(test, drm_constraints_list_suggest(list, selected ? id : 0), 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_list_observe(list, &generation), 0);
	return generation;
}

static void read_change(struct kunit *test, struct query_fixture *f, unsigned long address,
			u64 generation)
{
	struct drm_event_kms_constraints_list_changed event;
	loff_t offset = 0;

	KUNIT_ASSERT_EQ(test, drm_read(f->file, (char __user *)address, sizeof(event), &offset),
			sizeof(event));
	KUNIT_ASSERT_EQ(test, copy_from_user(&event, (void __user *)address, sizeof(event)), 0);
	KUNIT_EXPECT_EQ(test, event.base.type, DRM_EVENT_KMS_CONSTRAINTS_LIST_CHANGED);
	KUNIT_EXPECT_EQ(test, event.crtc_id, f->crtc->base.id);
	KUNIT_EXPECT_EQ(test, event.generation, generation);
}

static void subscription_pause_retains_one_bounded_producer(struct kunit *test)
{
	struct query_fixture *f = new_fixture(test);
	struct drm_file *priv = f->file->private_data;
	struct drm_constraints_events *events;
	unsigned long address = user_page(test);
	u64 first, latest = 0;
	unsigned int i;
	loff_t offset = 0;

	client_cap(test, f->file, address, DRM_CLIENT_CAP_ATOMIC, 1, 0);
	client_cap(test, f->file, address, DRM_CLIENT_CAP_KMS_CONSTRAINTS, 1, 0);
	events = priv->constraints_events;
	KUNIT_ASSERT_NOT_NULL(test, events);
	first = change_suggestion(test, f, true);
	drm_constraints_events_flush(events);
	for (i = 0; i < 32; i++) {
		client_cap(test, f->file, address, DRM_CLIENT_CAP_KMS_CONSTRAINTS, 0, 0);
		latest = change_suggestion(test, f, i & 1);
		client_cap(test, f->file, address, DRM_CLIENT_CAP_KMS_CONSTRAINTS, 1, 0);
		KUNIT_EXPECT_PTR_EQ(test, priv->constraints_events, events);
		drm_constraints_events_flush(events);
	}
	read_change(test, f, address, first);
	drm_constraints_events_flush(events);
	read_change(test, f, address, latest);
	client_cap(test, f->file, address, DRM_CLIENT_CAP_KMS_CONSTRAINTS, 0, 0);
	latest = change_suggestion(test, f, false);
	drm_constraints_events_flush(events);
	KUNIT_EXPECT_EQ(test,
			drm_read(f->file, (char __user *)address, PAGE_SIZE, &offset), -EAGAIN);
	client_cap(test, f->file, address, DRM_CLIENT_CAP_KMS_CONSTRAINTS, 1, 0);
	drm_constraints_events_flush(events);
	read_change(test, f, address, latest);
}

static int install_test_selection(struct drm_constraints_entry *entry, void *data)
{
	struct drm_crtc *crtc = data;
	struct drm_constraints_entry *previous = crtc->state->constraints;

	crtc->state->constraints = drm_constraints_entry_get(entry);
	drm_constraints_entry_put(previous);
	return 0;
}

/* Supply disabled accepted state to test file policy independently of decoding. */
static int select_test_entry(struct drm_crtc *crtc, struct drm_constraints_entry *entry)
{
	struct drm_modeset_acquire_ctx ctx;
	int ret;

	drm_modeset_acquire_init(&ctx, 0);
	for (;;) {
		ret = drm_modeset_lock_all_ctx(crtc->dev, &ctx);
		if (ret != -EDEADLK)
			break;
		ret = drm_modeset_backoff(&ctx);
		if (ret)
			break;
	}
	if (!ret)
		ret = drm_constraints_list_accept(drm_constraints_crtc_list(crtc), entry,
						  install_test_selection, crtc);
	drm_modeset_drop_locks(&ctx);
	drm_modeset_acquire_fini(&ctx);
	return ret;
}

static struct drm_constraints_entry *new_target(struct kunit *test, struct query_fixture *f)
{
	struct drm_constraints_entry *entry =
		drm_constraints_entry_create_stateless(drm_constraints_device_domain(f->dev),
			f->crtc->base.id,
			drm_constraints_entry_description(drm_constraints_crtc_default(f->crtc)));

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, entry);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_entry, entry), 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_crtc_add(f->crtc, entry), 0);
	return entry;
}

static void current_owner_cannot_disable_a_nondefault_contract(struct kunit *test)
{
	struct query_fixture *f = new_fixture(test);
	struct drm_file *priv = f->file->private_data;
	struct drm_constraints_entry *target = new_target(test, f);
	unsigned long address = user_page(test);

	client_cap(test, f->file, address, DRM_CLIENT_CAP_ATOMIC, 1, 0);
	client_cap(test, f->file, address, DRM_CLIENT_CAP_KMS_CONSTRAINTS, 1, 0);
	KUNIT_ASSERT_EQ(test, select_test_entry(f->crtc, target), 0);
	client_cap(test, f->file, address, DRM_CLIENT_CAP_KMS_CONSTRAINTS, 0, -EBUSY);
	KUNIT_EXPECT_TRUE(test, priv->kms_constraints);
	KUNIT_ASSERT_EQ(test, select_test_entry(f->crtc, drm_constraints_crtc_default(f->crtc)), 0);
	client_cap(test, f->file, address, DRM_CLIENT_CAP_KMS_CONSTRAINTS, 0, 0);
	KUNIT_EXPECT_FALSE(test, priv->kms_constraints);
}

static void departed_owner_can_disable_its_subscription(struct kunit *test)
{
	struct query_fixture *f = new_fixture(test);
	struct drm_file *priv = f->file->private_data;
	struct drm_constraints_entry *target = new_target(test, f);
	unsigned long address = user_page(test);

	client_cap(test, f->file, address, DRM_CLIENT_CAP_ATOMIC, 1, 0);
	client_cap(test, f->file, address, DRM_CLIENT_CAP_KMS_CONSTRAINTS, 1, 0);
	KUNIT_ASSERT_EQ(test, select_test_entry(f->crtc, target), 0);
	kunit_release_action(test, stop_owner, f->dev);
	KUNIT_ASSERT_EQ(test, drm_dropmaster_ioctl(f->dev, NULL, priv), 0);
	client_cap(test, f->file, address, DRM_CLIENT_CAP_KMS_CONSTRAINTS, 0, 0);
	KUNIT_EXPECT_FALSE(test, priv->kms_constraints);
}

static void file_close_discards_subscribed_records(struct kunit *test)
{
	struct query_fixture *f = new_fixture(test);
	struct drm_file *priv = f->file->private_data;
	unsigned long address = user_page(test);

	client_cap(test, f->file, address, DRM_CLIENT_CAP_ATOMIC, 1, 0);
	client_cap(test, f->file, address, DRM_CLIENT_CAP_KMS_CONSTRAINTS, 1, 0);
	KUNIT_ASSERT_NOT_NULL(test, priv->constraints_events);
	change_suggestion(test, f, true);
	drm_constraints_events_flush(priv->constraints_events);
	kunit_release_action(test, stop_owner, f->dev);
	kunit_release_action(test, close_file, f->file);
	f->file = NULL;
}

static int lock_request(struct drm_device *dev, struct drm_modeset_acquire_ctx *ctx)
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

static void retained_requests_recheck_client_opt_in(struct kunit *test)
{
	struct query_fixture *f = new_fixture(test);
	struct drm_file *priv = f->file->private_data;
	struct drm_constraints_entry *target = new_target(test, f);
	u32 object = f->crtc->base.id, count = 1;
	u32 property = f->dev->mode_config.prop_constraints_id->base.id;
	u64 value = drm_constraints_entry_id(target);
	const struct drm_atomic_user_input input = {
		.object_count = 1, .property_count = 1,
		.objects = &object, .counts = &count, .properties = &property, .values = &value,
	};
	struct drm_atomic_user_request *request;
	struct drm_modeset_acquire_ctx ctx;
	unsigned long address = user_page(test);
	int ret;

	client_cap(test, f->file, address, DRM_CLIENT_CAP_ATOMIC, 1, 0);
	KUNIT_ASSERT_EQ(test, lock_request(f->dev, &ctx), 0);
	request = drm_atomic_resolve_user_request(f->dev, priv, &input);
	drm_modeset_drop_locks(&ctx);
	drm_modeset_acquire_fini(&ctx);
	KUNIT_ASSERT_EQ(test, PTR_ERR(request), -EOPNOTSUPP);
	client_cap(test, f->file, address, DRM_CLIENT_CAP_KMS_CONSTRAINTS, 1, 0);
	KUNIT_ASSERT_EQ(test, lock_request(f->dev, &ctx), 0);
	request = drm_atomic_resolve_user_request(f->dev, priv, &input);
	drm_modeset_drop_locks(&ctx);
	drm_modeset_acquire_fini(&ctx);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, request);
	/* Preparation may wait without locks while the issuing file opts out. */
	client_cap(test, f->file, address, DRM_CLIENT_CAP_KMS_CONSTRAINTS, 0, 0);
	KUNIT_ASSERT_EQ(test, lock_request(f->dev, &ctx), 0);
	ret = drm_atomic_validate_user_request(request, priv);
	drm_modeset_drop_locks(&ctx);
	drm_modeset_acquire_fini(&ctx);
	KUNIT_EXPECT_EQ(test, ret, -EOPNOTSUPP);
	client_cap(test, f->file, address, DRM_CLIENT_CAP_KMS_CONSTRAINTS, 1, 0);
	KUNIT_ASSERT_EQ(test, lock_request(f->dev, &ctx), 0);
	ret = drm_atomic_validate_user_request(request, priv);
	drm_modeset_drop_locks(&ctx);
	drm_modeset_acquire_fini(&ctx);
	KUNIT_EXPECT_EQ(test, ret, 0);
	drm_atomic_free_user_request(request);
}

static int check_atomic(struct drm_device *dev, struct drm_atomic_commit *state)
{
	return 0;
}

static int install_atomic(struct drm_device *dev, struct drm_atomic_commit *state, bool nonblock)
{
	return drm_atomic_helper_swap_state(state, false);
}

static const struct drm_mode_config_funcs atomic_ops = {
	.atomic_check = check_atomic,
	.atomic_commit = install_atomic,
};

static void atomic_ioctl_selects_only_on_acceptance(struct kunit *test)
{
	struct query_fixture *f = new_fixture(test);
	struct drm_constraints_entry *target = new_target(test, f);
	struct drm_constraints_entry *initial = drm_constraints_crtc_default(f->crtc);
	unsigned long address = user_page(test);
	struct {
		struct drm_mode_atomic arg;
		u32 object, count, property, pad;
		u64 value;
	} input = {
		.arg = {
			.flags = DRM_MODE_ATOMIC_TEST_ONLY | DRM_MODE_ATOMIC_ALLOW_MODESET,
			.count_objs = 1,
		},
		.object = f->crtc->base.id, .count = 1,
		.property = f->dev->mode_config.prop_constraints_id->base.id,
		.value = drm_constraints_entry_id(target),
	};
	long ret;

	f->dev->mode_config.funcs = &atomic_ops;
	input.arg.objs_ptr = address + offsetof(typeof(input), object);
	input.arg.count_props_ptr = address + offsetof(typeof(input), count);
	input.arg.props_ptr = address + offsetof(typeof(input), property);
	input.arg.prop_values_ptr = address + offsetof(typeof(input), value);
	client_cap(test, f->file, address, DRM_CLIENT_CAP_ATOMIC, 1, 0);
	KUNIT_ASSERT_EQ(test, copy_to_user((void __user *)address, &input, sizeof(input)), 0);
	ret = f->file->f_op->unlocked_ioctl(f->file, DRM_IOCTL_MODE_ATOMIC, address);
	KUNIT_EXPECT_EQ(test, ret, -EOPNOTSUPP);
	client_cap(test, f->file, address, DRM_CLIENT_CAP_KMS_CONSTRAINTS, 1, 0);
	KUNIT_ASSERT_EQ(test, copy_to_user((void __user *)address, &input, sizeof(input)), 0);
	ret = f->file->f_op->unlocked_ioctl(f->file, DRM_IOCTL_MODE_ATOMIC, address);
	KUNIT_ASSERT_EQ(test, ret, 0);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, initial);
	input.arg.flags = DRM_MODE_ATOMIC_ALLOW_MODESET;
	KUNIT_ASSERT_EQ(test, copy_to_user((void __user *)address, &input, sizeof(input)), 0);
	ret = f->file->f_op->unlocked_ioctl(f->file, DRM_IOCTL_MODE_ATOMIC, address);
	KUNIT_ASSERT_EQ(test, ret, 0);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, target);
	/* Repeating the accepted ID needs no permission to change modes. */
	input.arg.flags = 0;
	KUNIT_ASSERT_EQ(test, copy_to_user((void __user *)address, &input, sizeof(input)), 0);
	ret = f->file->f_op->unlocked_ioctl(f->file, DRM_IOCTL_MODE_ATOMIC, address);
	KUNIT_EXPECT_EQ(test, ret, 0);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, target);
}

static struct kunit_case constraints_uapi_cases[] = {
	KUNIT_CASE(atomic_ioctl_selects_only_on_acceptance),
	KUNIT_CASE(retained_requests_recheck_client_opt_in),
	KUNIT_CASE(ioctl_publishes_sizing_metadata_on_enospc),
	KUNIT_CASE(ioctl_checks_master_and_output_scope),
	KUNIT_CASE(ioctl_respects_lease_visibility),
	KUNIT_CASE(ioctl_encoding_requires_modesetting_master),
	KUNIT_CASE(subscription_requires_atomic_support),
	KUNIT_CASE(unsupported_devices_do_not_create_subscriptions),
	KUNIT_CASE(outputless_devices_do_not_publish_opt_in),
	KUNIT_CASE(subscription_pause_retains_one_bounded_producer),
	KUNIT_CASE(current_owner_cannot_disable_a_nondefault_contract),
	KUNIT_CASE(departed_owner_can_disable_its_subscription),
	KUNIT_CASE(file_close_discards_subscribed_records),
	{}
};

static struct kunit_suite constraints_uapi_suite = {
	.name = "drm_constraints_uapi",
	.test_cases = constraints_uapi_cases,
};
kunit_test_suite(constraints_uapi_suite);

MODULE_LICENSE("GPL and additional rights");
