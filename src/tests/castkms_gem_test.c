// SPDX-License-Identifier: GPL-2.0-only

#include <linux/dma-buf.h>
#include <linux/err.h>
#include <linux/fcntl.h>

#include <kunit/test.h>

#include <drm/drm_device.h>
#include <drm/drm_gem.h>
#include <drm/drm_gem_shmem_helper.h>
#include <drm/drm_prime.h>

#include "../castkms_config.h"
#include "../castkms_device.h"
#include "../castkms_gem.h"

MODULE_IMPORT_NS("EXPORTED_FOR_KUNIT_TESTING");
MODULE_IMPORT_NS("DMA_BUF");

struct castkms_gem_test_context {
	struct castkms_config *config;
};

static void castkms_gem_test_put_object(void *data)
{
	drm_gem_object_put(data);
}

static void castkms_gem_test_put_dma_buf(void *data)
{
	dma_buf_put(data);
}

static void castkms_gem_test_cleanup(void *data)
{
	struct castkms_gem_test_context *context = data;

	if (context->config->dev)
		castkms_destroy(context->config);
	castkms_config_destroy(context->config);
}

static int castkms_gem_test_init(struct kunit *test)
{
	struct castkms_gem_test_context *context;
	int ret;

	context = kunit_kzalloc(test, sizeof(*context), GFP_KERNEL);
	if (!context)
		return -ENOMEM;
	test->priv = context;

	context->config = castkms_config_default_create(false, false, false,
							false);
	if (IS_ERR(context->config))
		return PTR_ERR(context->config);

	ret = kunit_add_action_or_reset(test, castkms_gem_test_cleanup, context);
	if (ret)
		return ret;

	return castkms_create(context->config);
}

static struct dma_buf *
castkms_gem_test_export_object(struct kunit *test,
			       struct drm_gem_object **object_out)
{
	struct castkms_gem_test_context *context = test->priv;
	struct drm_gem_shmem_object *shmem;
	struct dma_buf *dma_buf;
	int ret;

	shmem = drm_gem_shmem_create(&context->config->dev->drm, PAGE_SIZE);
	if (IS_ERR(shmem))
		return ERR_CAST(shmem);

	ret = kunit_add_action_or_reset(test, castkms_gem_test_put_object,
					&shmem->base);
	if (ret)
		return ERR_PTR(ret);

	dma_buf = drm_gem_prime_export(&shmem->base, O_RDWR);
	if (IS_ERR(dma_buf))
		return dma_buf;

	ret = kunit_add_action_or_reset(test, castkms_gem_test_put_dma_buf,
					dma_buf);
	if (ret)
		return ERR_PTR(ret);

	*object_out = &shmem->base;
	return dma_buf;
}

static void castkms_gem_test_allows_same_device_import(struct kunit *test)
{
	struct castkms_gem_test_context *context = test->priv;
	struct drm_gem_object *exported;
	struct drm_gem_object *imported;
	struct dma_buf *dma_buf;
	int ret;

	dma_buf = castkms_gem_test_export_object(test, &exported);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, dma_buf);

	imported = castkms_gem_prime_import(&context->config->dev->drm, dma_buf);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, imported);
	ret = kunit_add_action_or_reset(test, castkms_gem_test_put_object,
					imported);
	KUNIT_ASSERT_EQ(test, ret, 0);

	KUNIT_EXPECT_PTR_EQ(test, imported, exported);
}

static void castkms_gem_test_rejects_foreign_import(struct kunit *test)
{
	struct drm_device *foreign_device;
	struct drm_gem_object *exported;
	struct drm_gem_object *imported;
	struct dma_buf *dma_buf;

	dma_buf = castkms_gem_test_export_object(test, &exported);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, dma_buf);

	foreign_device = kunit_kzalloc(test, sizeof(*foreign_device), GFP_KERNEL);
	KUNIT_ASSERT_NOT_NULL(test, foreign_device);
	imported = castkms_gem_prime_import(foreign_device, dma_buf);

	KUNIT_ASSERT_TRUE(test, IS_ERR(imported));
	KUNIT_EXPECT_EQ(test, PTR_ERR(imported), -EOPNOTSUPP);
}

static struct kunit_case castkms_gem_test_cases[] = {
	KUNIT_CASE(castkms_gem_test_allows_same_device_import),
	KUNIT_CASE(castkms_gem_test_rejects_foreign_import),
	{}
};

static struct kunit_suite castkms_gem_test_suite = {
	.name = "castkms-gem",
	.init = castkms_gem_test_init,
	.test_cases = castkms_gem_test_cases,
};

kunit_test_suite(castkms_gem_test_suite);

MODULE_DESCRIPTION("KUnit tests for CastKMS PRIME import policy");
MODULE_LICENSE("GPL");
