// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/dma-buf.h>
#include <linux/err.h>
#include <linux/file.h>
#include <linux/module.h>
#include <drm/drm_capture_destination.h>
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

static struct kunit_case destination_cases[] = {
	KUNIT_CASE(drm_capture_destination_accepts_borrowed_writable_storage),
	KUNIT_CASE(drm_capture_destination_checks_every_plane_without_forbidding_aliases),
	KUNIT_CASE(drm_capture_destination_rejects_invalid_description_shape),
	KUNIT_CASE(drm_capture_destination_checks_offsets_without_claiming_complete_rows),
	{}
};

static struct kunit_suite destination_suite = {
	.name = "drm_capture_destination",
	.test_cases = destination_cases,
};

kunit_test_suite(destination_suite);

MODULE_LICENSE("GPL");
MODULE_IMPORT_NS("DMA_BUF");
