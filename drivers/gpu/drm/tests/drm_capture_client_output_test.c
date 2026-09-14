// SPDX-License-Identifier: GPL-2.0-only

#include <linux/dma-fence.h>
#include <linux/err.h>
#include <linux/file.h>
#include <linux/module.h>
#include <drm/drm_capture_authority.h>
#include <drm/drm_capture_file.h>
#include <drm/drm_capture_readiness.h>
#include <kunit/test.h>

struct output_context {
	struct drm_capture_readiness *readiness;
	struct dma_fence *reuse;
	u64 stream;
	u64 use_id;
	u64 destination;
	unsigned int calls;
	int status;
};

static void output_release(void *data)
{
	struct output_context *context = data;

	dma_fence_put(context->reuse);
}

static void output_revoke(void *data)
{
}

static struct drm_capture_readiness *output_get_readiness(void *data)
{
	struct output_context *context = data;

	return drm_capture_readiness_get(context->readiness);
}

static int output_close(void *data, u64 stream)
{
	return 0;
}

static int output_dequeue(void *data, u64 stream, const struct drm_capture_completion_sink *sink)
{
	return -EAGAIN;
}

static int output_queue(void *data, u64 stream, u64 use_id, u64 destination,
			struct dma_fence *reuse)
{
	struct output_context *context = data;

	context->calls++;
	if (context->status)
		return context->status;
	context->stream = stream;
	context->use_id = use_id;
	context->destination = destination;
	dma_fence_put(context->reuse);
	context->reuse = dma_fence_get(reuse);
	return 0;
}

static const struct drm_capture_authority_ops output_authority_ops = {
	.owner = THIS_MODULE,
	.revoke = output_revoke,
};

static const struct drm_capture_client_owner_ops output_ops = {
	.owner = THIS_MODULE,
	.release = output_release,
	.get_readiness = output_get_readiness,
	.close_stream = output_close,
	.dequeue = output_dequeue,
	.queue_output = output_queue,
};

static const struct drm_capture_client_owner_ops output_incomplete_ops[] = {
	{
		.owner = THIS_MODULE,
		.release = output_release,
		.close_stream = output_close,
		.dequeue = output_dequeue,
		.queue_output = output_queue,
	}, {
		.owner = THIS_MODULE,
		.release = output_release,
		.get_readiness = output_get_readiness,
		.dequeue = output_dequeue,
		.queue_output = output_queue,
	}, {
		.owner = THIS_MODULE,
		.release = output_release,
		.get_readiness = output_get_readiness,
		.close_stream = output_close,
		.queue_output = output_queue,
	}, {
		.owner = THIS_MODULE,
		.release = output_release,
		.get_readiness = output_get_readiness,
		.close_stream = output_close,
		.dequeue = output_dequeue,
	},
};

static void output_put_authority(void *authority)
{
	drm_capture_authority_put(authority);
}

static void output_put_file(void *file)
{
	__fput_sync(file);
}

static void output_put_readiness(void *readiness)
{
	drm_capture_readiness_put(readiness);
}

static void output_put_fence(void *fence)
{
	dma_fence_put(fence);
}

static struct file *output_create(struct kunit *test,
				  const struct drm_capture_client_owner_ops *ops,
				  struct drm_capture_authority **authority,
				  struct output_context **context)
{
	struct file *file;
	int ret;

	*context = kunit_kzalloc(test, sizeof(**context), GFP_KERNEL);
	KUNIT_ASSERT_NOT_NULL(test, *context);
	(*context)->readiness = drm_capture_readiness_create();
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, (*context)->readiness);
	ret = kunit_add_action_or_reset(test, output_put_readiness, (*context)->readiness);
	KUNIT_ASSERT_EQ(test, ret, 0);
	*authority = drm_capture_authority_create(&output_authority_ops, *context);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, *authority);
	KUNIT_ASSERT_EQ(test,
			kunit_add_action_or_reset(test, output_put_authority, *authority), 0);
	file = drm_capture_client_file_create(*authority, ops, *context);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, file);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, output_put_file, file), 0);
	return file;
}

static void drm_capture_client_output_retains_borrowed_submission_inputs(struct kunit *test)
{
	struct drm_capture_authority *authority;
	struct output_context *context;
	struct file *file = output_create(test, &output_ops, &authority, &context);
	struct dma_fence *reuse = dma_fence_allocate_private_stub(0);

	KUNIT_ASSERT_NOT_NULL(test, reuse);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, output_put_fence, reuse), 0);
	KUNIT_EXPECT_EQ(test, drm_capture_client_queue_output(file, 7, 19, 23, reuse), 0);
	KUNIT_EXPECT_EQ(test, context->calls, 1);
	KUNIT_EXPECT_EQ(test, context->stream, 7);
	KUNIT_EXPECT_EQ(test, context->use_id, 19);
	KUNIT_EXPECT_EQ(test, context->destination, 23);
	KUNIT_EXPECT_PTR_EQ(test, context->reuse, reuse);
	kunit_release_action(test, output_put_fence, reuse);
	KUNIT_ASSERT_NOT_NULL(test, context->reuse);
	KUNIT_EXPECT_EQ(test, dma_fence_get_status(context->reuse), 1);
}

static void drm_capture_client_output_checks_arguments_and_revocation(struct kunit *test)
{
	struct drm_capture_authority *authority;
	struct output_context *context;
	struct file *file = output_create(test, &output_ops, &authority, &context);
	struct file *control = drm_capture_control_file_create(authority);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, control);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, output_put_file, control), 0);
	KUNIT_EXPECT_EQ(test, drm_capture_client_queue_output(NULL, 7, 19, 23, NULL), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_capture_client_queue_output(control, 7, 19, 23, NULL), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_capture_client_queue_output(file, 0, 19, 23, NULL), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_capture_client_queue_output(file, 7, 0, 23, NULL), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_capture_client_queue_output(file, 7, 19, 0, NULL), -EINVAL);
	drm_capture_authority_revoke(authority);
	KUNIT_EXPECT_EQ(test, drm_capture_client_queue_output(file, 7, 19, 23, NULL), -EKEYREVOKED);
	KUNIT_EXPECT_EQ(test, context->calls, 0);
}

static void drm_capture_client_output_requires_completion_and_cleanup_support(struct kunit *test)
{
	unsigned int i;

	for (i = 0; i < ARRAY_SIZE(output_incomplete_ops); i++) {
		struct drm_capture_authority *authority;
		struct output_context *context;
		struct file *file;

		file = output_create(test, &output_incomplete_ops[i], &authority, &context);

		KUNIT_EXPECT_EQ(test, drm_capture_client_queue_output(file, 7, 19, 23, NULL),
				-EOPNOTSUPP);
		KUNIT_EXPECT_EQ(test, context->calls, 0);
	}
}

static void drm_capture_client_output_preserves_provider_failure_and_retry(struct kunit *test)
{
	struct drm_capture_authority *authority;
	struct output_context *context;
	struct file *file = output_create(test, &output_ops, &authority, &context);

	context->status = -EAGAIN;
	KUNIT_EXPECT_EQ(test, drm_capture_client_queue_output(file, 7, 19, 23, NULL), -EAGAIN);
	KUNIT_EXPECT_EQ(test, context->use_id, 0);
	context->status = 1;
	KUNIT_EXPECT_EQ(test, drm_capture_client_queue_output(file, 7, 19, 23, NULL), -EINVAL);
	KUNIT_EXPECT_EQ(test, context->use_id, 0);
	context->status = 0;
	KUNIT_EXPECT_EQ(test, drm_capture_client_queue_output(file, 7, 19, 23, NULL), 0);
	KUNIT_EXPECT_EQ(test, context->use_id, 19);
	KUNIT_EXPECT_EQ(test, context->calls, 3);
	KUNIT_EXPECT_PTR_EQ(test, context->reuse, NULL);
}

static struct kunit_case drm_capture_client_output_cases[] = {
	KUNIT_CASE(drm_capture_client_output_retains_borrowed_submission_inputs),
	KUNIT_CASE(drm_capture_client_output_checks_arguments_and_revocation),
	KUNIT_CASE(drm_capture_client_output_requires_completion_and_cleanup_support),
	KUNIT_CASE(drm_capture_client_output_preserves_provider_failure_and_retry),
	{}
};

static struct kunit_suite drm_capture_client_output_suite = {
	.name = "drm_capture_client_output",
	.test_cases = drm_capture_client_output_cases,
};

kunit_test_suite(drm_capture_client_output_suite);
MODULE_LICENSE("GPL");
