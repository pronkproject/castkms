// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/dma-buf.h>
#include <linux/err.h>
#include <linux/file.h>
#include <linux/module.h>
#include <drm/drm_capture_authority.h>
#include <drm/drm_capture_destination.h>
#include <drm/drm_capture_file.h>
#include <drm/drm_fourcc.h>
#include <kunit/test.h>

static struct sg_table *destination_map(struct dma_buf_attachment *attachment,
					    enum dma_data_direction direction)
{
	return ERR_PTR(-EOPNOTSUPP);
}

static void destination_unmap(struct dma_buf_attachment *attachment,
			      struct sg_table *table, enum dma_data_direction direction)
{
}

static void destination_release(struct dma_buf *buffer)
{
}

static const struct dma_buf_ops destination_ops = {
	.map_dma_buf = destination_map,
	.unmap_dma_buf = destination_unmap,
	.release = destination_release,
};

static void destination_put(void *buffer)
{
	dma_buf_put(buffer);
	flush_delayed_fput();
}

static struct dma_buf *destination_buffer(struct kunit *test, int flags)
{
	DEFINE_DMA_BUF_EXPORT_INFO(info);
	struct dma_buf *buffer;

	info.ops = &destination_ops;
	info.size = 4096;
	info.flags = flags;
	info.priv = test;
	buffer = dma_buf_export(&info);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, buffer);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, destination_put, buffer), 0);
	return buffer;
}

static struct drm_capture_destination destination_description(struct dma_buf *buffer)
{
	return (struct drm_capture_destination) {
		.width = 16,
		.height = 16,
		.format = DRM_FORMAT_XRGB8888,
		.num_planes = 1,
		.modifier = DRM_FORMAT_MOD_LINEAR,
		.planes = { { .buffer = buffer, .stride = 64 } },
	};
}

static void drm_capture_destination_accepts_borrowed_writable_storage(struct kunit *test)
{
	struct drm_capture_destination destination =
		destination_description(destination_buffer(test, O_RDWR));
	struct drm_capture_destination original = destination;

	KUNIT_EXPECT_EQ(test, drm_capture_destination_validate(&destination), 0);
	KUNIT_EXPECT_MEMEQ(test, &destination, &original, sizeof(destination));
}

static void drm_capture_destination_checks_every_plane_without_forbidding_aliases(struct kunit *test)
{
	struct drm_capture_destination destination =
		destination_description(destination_buffer(test, O_WRONLY));

	destination.num_planes = 2;
	destination.planes[1] = destination.planes[0];
	destination.planes[1].offset = 1024;
	/* Format-specific plane count and layout validation belong to the provider. */
	KUNIT_EXPECT_EQ(test, drm_capture_destination_validate(&destination), 0);
	destination.planes[1].buffer = destination_buffer(test, O_RDONLY);
	KUNIT_EXPECT_EQ(test, drm_capture_destination_validate(&destination), -EACCES);
}

static void drm_capture_destination_rejects_invalid_description_shape(struct kunit *test)
{
	struct drm_capture_destination valid =
		destination_description(destination_buffer(test, O_RDWR));
	struct drm_capture_destination invalid;

	KUNIT_EXPECT_EQ(test, drm_capture_destination_validate(NULL), -EINVAL);
	invalid = valid;
	invalid.width = 0;
	KUNIT_EXPECT_EQ(test, drm_capture_destination_validate(&invalid), -EINVAL);
	invalid = valid;
	invalid.height = 0;
	KUNIT_EXPECT_EQ(test, drm_capture_destination_validate(&invalid), -EINVAL);
	invalid = valid;
	invalid.format = 0;
	KUNIT_EXPECT_EQ(test, drm_capture_destination_validate(&invalid), -EINVAL);
	invalid = valid;
	invalid.modifier = DRM_FORMAT_MOD_INVALID;
	KUNIT_EXPECT_EQ(test, drm_capture_destination_validate(&invalid), -EINVAL);
	invalid = valid;
	invalid.num_planes = 0;
	KUNIT_EXPECT_EQ(test, drm_capture_destination_validate(&invalid), -EINVAL);
	invalid.num_planes = DRM_CAPTURE_DESTINATION_MAX_PLANES + 1;
	KUNIT_EXPECT_EQ(test, drm_capture_destination_validate(&invalid), -EINVAL);
}

static void drm_capture_destination_checks_offsets_without_claiming_complete_rows(struct kunit *test)
{
	struct drm_capture_destination destination =
		destination_description(destination_buffer(test, O_RDWR));

	destination.planes[0].offset = 4095;
	KUNIT_EXPECT_EQ(test, drm_capture_destination_validate(&destination), 0);
	destination.planes[0].offset = 4096;
	KUNIT_EXPECT_EQ(test, drm_capture_destination_validate(&destination), -EINVAL);
	destination.planes[0].offset = U64_MAX;
	KUNIT_EXPECT_EQ(test, drm_capture_destination_validate(&destination), -EINVAL);
	destination.planes[0].offset = 0;
	destination.planes[0].stride = 0;
	KUNIT_EXPECT_EQ(test, drm_capture_destination_validate(&destination), -EINVAL);
	destination.planes[0].stride = 64;
	destination.planes[0].buffer = NULL;
	KUNIT_EXPECT_EQ(test, drm_capture_destination_validate(&destination), -EINVAL);
}

struct destination_client {
	struct dma_buf *retained;
	u64 last_id;
	unsigned int registrations;
	unsigned int removals;
	int register_result;
	int remove_result;
};

static void destination_client_revoke(void *data)
{
}

static void destination_client_release(void *data)
{
	struct destination_client *client = data;

	if (client->retained)
		dma_buf_put(client->retained);
	client->retained = NULL;
}

static int destination_client_register(void *data, u64 id,
				       const struct drm_capture_destination *destination)
{
	struct destination_client *client = data;

	client->registrations++;
	if (client->register_result)
		return client->register_result;
	if (id <= client->last_id)
		return -ESTALE;
	if (client->retained)
		return -EBUSY;
	if (destination->num_planes != 1)
		return -EOPNOTSUPP;
	client->retained = destination->planes[0].buffer;
	get_dma_buf(client->retained);
	client->last_id = id;
	return 0;
}

static int destination_client_unregister(void *data, u64 id)
{
	struct destination_client *client = data;

	client->removals++;
	if (client->remove_result)
		return client->remove_result;
	if (id != client->last_id || !client->retained)
		return -ENOENT;
	destination_client_release(client);
	return 0;
}

static const struct drm_capture_authority_ops destination_authority_ops = {
	.owner = THIS_MODULE,
	.revoke = destination_client_revoke,
};

static const struct drm_capture_client_owner_ops destination_client_ops = {
	.owner = THIS_MODULE,
	.release = destination_client_release,
	.register_destination = destination_client_register,
	.unregister_destination = destination_client_unregister,
};

static const struct drm_capture_client_owner_ops destination_no_remove_ops = {
	.owner = THIS_MODULE,
	.release = destination_client_release,
	.register_destination = destination_client_register,
};

static void destination_put_authority(void *authority)
{
	drm_capture_authority_put(authority);
}

static void destination_put_file(void *file)
{
	__fput_sync(file);
	flush_delayed_fput();
}

static struct file *destination_client_file(struct kunit *test,
					     struct drm_capture_authority **authority,
					     struct destination_client **client,
					     const struct drm_capture_client_owner_ops *ops)
{
	struct file *file;

	*client = kunit_kzalloc(test, sizeof(**client), GFP_KERNEL);
	KUNIT_ASSERT_NOT_NULL(test, *client);
	*authority = drm_capture_authority_create(&destination_authority_ops, *client);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, *authority);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, destination_put_authority, *authority), 0);
	file = drm_capture_client_file_create(*authority, ops, *client);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, file);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, destination_put_file, file), 0);
	return file;
}

static void drm_capture_destination_client_retains_exact_storage(struct kunit *test)
{
	struct drm_capture_authority *authority;
	struct destination_client *client;
	struct file *file = destination_client_file(test, &authority, &client, &destination_client_ops);
	struct drm_capture_destination destination =
		destination_description(destination_buffer(test, O_RDWR));

	KUNIT_ASSERT_EQ(test, drm_capture_client_register_destination(file, 7, &destination), 0);
	KUNIT_EXPECT_PTR_EQ(test, client->retained, destination.planes[0].buffer);
	KUNIT_EXPECT_EQ(test, drm_capture_client_register_destination(file, 8, &destination), -EBUSY);
	KUNIT_EXPECT_EQ(test, drm_capture_client_unregister_destination(file, 8), -ENOENT);
	KUNIT_ASSERT_EQ(test, drm_capture_client_unregister_destination(file, 7), 0);
	KUNIT_EXPECT_EQ(test, drm_capture_client_register_destination(file, 7, &destination), -ESTALE);
	KUNIT_EXPECT_EQ(test, drm_capture_client_register_destination(file, 8, &destination), 0);
}

static void drm_capture_destination_client_checks_role_metadata_and_cleanup_support(struct kunit *test)
{
	struct drm_capture_authority *authority;
	struct destination_client *client;
	struct file *file = destination_client_file(test, &authority, &client, &destination_no_remove_ops);
	struct drm_capture_destination destination =
		destination_description(destination_buffer(test, O_RDWR));
	struct file *control;

	KUNIT_EXPECT_EQ(test, drm_capture_client_register_destination(NULL, 1, &destination), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_capture_client_unregister_destination(NULL, 1), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_capture_client_register_destination(file, 0, &destination), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_capture_client_unregister_destination(file, 0), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_capture_client_register_destination(file, 1, NULL), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_capture_client_register_destination(file, 1, &destination), -EOPNOTSUPP);
	KUNIT_EXPECT_EQ(test, drm_capture_client_unregister_destination(file, 1), -EOPNOTSUPP);
	control = drm_capture_control_file_create(authority);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, control);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, destination_put_file, control), 0);
	KUNIT_EXPECT_EQ(test, drm_capture_client_register_destination(control, 1, &destination), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_capture_client_unregister_destination(control, 1), -EINVAL);
	KUNIT_EXPECT_EQ(test, client->registrations, 0);
}

static void drm_capture_destination_client_rejects_read_only_before_provider_admission(struct kunit *test)
{
	struct drm_capture_authority *authority;
	struct destination_client *client;
	struct file *file = destination_client_file(test, &authority, &client, &destination_client_ops);
	struct drm_capture_destination destination =
		destination_description(destination_buffer(test, O_RDONLY));

	KUNIT_EXPECT_EQ(test, drm_capture_client_register_destination(file, 1, &destination), -EACCES);
	KUNIT_EXPECT_EQ(test, client->registrations, 0);
	KUNIT_EXPECT_PTR_EQ(test, client->retained, NULL);
}

static void drm_capture_destination_client_revocation_preserves_cleanup(struct kunit *test)
{
	struct drm_capture_authority *authority;
	struct destination_client *client;
	struct file *file = destination_client_file(test, &authority, &client, &destination_client_ops);
	struct drm_capture_destination destination =
		destination_description(destination_buffer(test, O_RDWR));

	KUNIT_ASSERT_EQ(test, drm_capture_client_register_destination(file, 1, &destination), 0);
	drm_capture_authority_revoke(authority);
	KUNIT_EXPECT_EQ(test, drm_capture_client_register_destination(file, 2, &destination), -EKEYREVOKED);
	KUNIT_EXPECT_EQ(test, client->registrations, 1);
	KUNIT_ASSERT_EQ(test, drm_capture_client_unregister_destination(file, 1), 0);
	KUNIT_EXPECT_PTR_EQ(test, client->retained, NULL);
}

static void drm_capture_destination_client_preserves_provider_errors(struct kunit *test)
{
	struct drm_capture_authority *authority;
	struct destination_client *client;
	struct file *file = destination_client_file(test, &authority, &client, &destination_client_ops);
	struct drm_capture_destination destination =
		destination_description(destination_buffer(test, O_RDWR));

	client->register_result = -ENOMEM;
	KUNIT_EXPECT_EQ(test, drm_capture_client_register_destination(file, 1, &destination), -ENOMEM);
	client->register_result = 1;
	KUNIT_EXPECT_EQ(test, drm_capture_client_register_destination(file, 1, &destination), -EINVAL);
	KUNIT_EXPECT_EQ(test, client->last_id, 0);
	client->register_result = 0;
	KUNIT_ASSERT_EQ(test, drm_capture_client_register_destination(file, 1, &destination), 0);
	client->remove_result = -EIO;
	KUNIT_EXPECT_EQ(test, drm_capture_client_unregister_destination(file, 1), -EIO);
	KUNIT_EXPECT_NOT_NULL(test, client->retained);
	client->remove_result = 1;
	KUNIT_EXPECT_EQ(test, drm_capture_client_unregister_destination(file, 1), -EINVAL);
	client->remove_result = 0;
	KUNIT_ASSERT_EQ(test, drm_capture_client_unregister_destination(file, 1), 0);
}

static struct kunit_case destination_cases[] = {
	KUNIT_CASE(drm_capture_destination_accepts_borrowed_writable_storage),
	KUNIT_CASE(drm_capture_destination_checks_every_plane_without_forbidding_aliases),
	KUNIT_CASE(drm_capture_destination_rejects_invalid_description_shape),
	KUNIT_CASE(drm_capture_destination_checks_offsets_without_claiming_complete_rows),
	KUNIT_CASE(drm_capture_destination_client_retains_exact_storage),
	KUNIT_CASE(drm_capture_destination_client_checks_role_metadata_and_cleanup_support),
	KUNIT_CASE(drm_capture_destination_client_rejects_read_only_before_provider_admission),
	KUNIT_CASE(drm_capture_destination_client_revocation_preserves_cleanup),
	KUNIT_CASE(drm_capture_destination_client_preserves_provider_errors),
	{}
};

static struct kunit_suite destination_suite = {
	.name = "drm_capture_destination",
	.test_cases = destination_cases,
};

kunit_test_suite(destination_suite);

MODULE_LICENSE("GPL");
MODULE_IMPORT_NS("DMA_BUF");
