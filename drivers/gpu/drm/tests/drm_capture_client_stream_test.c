// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/err.h>
#include <linux/file.h>
#include <linux/module.h>
#include <drm/drm_capture_authority.h>
#include <drm/drm_capture_file.h>
#include <kunit/test.h>

struct client_stream_context {
	u64 id;
	u64 offer;
	u32 capacity;
	unsigned int opens;
	unsigned int closes;
	int open_result;
	int close_result;
	bool active;
};

static void client_stream_revoke(void *data)
{
}

static void client_stream_release(void *data)
{
	struct client_stream_context *context = data;

	context->active = false;
}

static int client_stream_open(void *data, u64 id, u64 offer, u32 capacity)
{
	struct client_stream_context *context = data;

	context->opens++;
	if (context->open_result)
		return context->open_result;
	if (id <= context->id)
		return -ESTALE;
	if (context->active)
		return -EBUSY;
	context->id = id;
	context->offer = offer;
	context->capacity = capacity;
	context->active = true;
	return 0;
}

static int client_stream_close(void *data, u64 id)
{
	struct client_stream_context *context = data;

	context->closes++;
	if (context->close_result)
		return context->close_result;
	if (id != context->id || !context->active)
		return -ENOENT;
	context->active = false;
	return 0;
}

static const struct drm_capture_authority_ops client_stream_authority_ops = {
	.owner = THIS_MODULE,
	.revoke = client_stream_revoke,
};

static const struct drm_capture_client_owner_ops client_stream_ops = {
	.owner = THIS_MODULE,
	.release = client_stream_release,
	.open_stream = client_stream_open,
	.close_stream = client_stream_close,
};

static const struct drm_capture_client_owner_ops client_stream_absent_ops = {
	.owner = THIS_MODULE,
	.release = client_stream_release,
};

static const struct drm_capture_client_owner_ops client_stream_no_close_ops = {
	.owner = THIS_MODULE,
	.release = client_stream_release,
	.open_stream = client_stream_open,
};

static void client_stream_put_authority(void *authority)
{
	drm_capture_authority_put(authority);
}

static void client_stream_put_file(void *file)
{
	__fput_sync(file);
}

static struct file *client_stream_create(struct kunit *test,
					 struct drm_capture_authority **authority,
					 struct client_stream_context **context)
{
	struct file *file;

	*context = kunit_kzalloc(test, sizeof(**context), GFP_KERNEL);
	KUNIT_ASSERT_NOT_NULL(test, *context);
	*authority = drm_capture_authority_create(&client_stream_authority_ops, *context);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, *authority);
	KUNIT_ASSERT_EQ(test,
		kunit_add_action_or_reset(test, client_stream_put_authority, *authority), 0);
	file = drm_capture_client_file_create(*authority, &client_stream_ops, *context);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, file);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, client_stream_put_file, file), 0);
	return file;
}

static void drm_capture_client_stream_dispatches_named_operations(struct kunit *test)
{
	struct drm_capture_authority *authority;
	struct client_stream_context *context;
	struct file *file = client_stream_create(test, &authority, &context);

	KUNIT_ASSERT_EQ(test, drm_capture_client_open_stream(file, 7, 19, 2), 0);
	KUNIT_EXPECT_EQ(test, context->id, 7);
	KUNIT_EXPECT_EQ(test, context->offer, 19);
	KUNIT_EXPECT_EQ(test, context->capacity, 2);
	KUNIT_EXPECT_EQ(test, drm_capture_client_open_stream(file, 8, 19, 2), -EBUSY);
	KUNIT_EXPECT_EQ(test, drm_capture_client_close_stream(file, 8), -ENOENT);
	KUNIT_ASSERT_EQ(test, drm_capture_client_close_stream(file, 7), 0);
	KUNIT_EXPECT_FALSE(test, context->active);
	KUNIT_EXPECT_EQ(test, drm_capture_client_open_stream(file, 7, 19, 2), -ESTALE);
	KUNIT_ASSERT_EQ(test, drm_capture_client_open_stream(file, 8, 19, 2), 0);
	KUNIT_EXPECT_TRUE(test, context->active);
}

static void drm_capture_client_stream_rejects_invalid_endpoints_and_numbers(struct kunit *test)
{
	struct drm_capture_authority *authority;
	struct client_stream_context *context;
	struct file *file = client_stream_create(test, &authority, &context);
	struct file *control, *absent;

	KUNIT_EXPECT_EQ(test, drm_capture_client_open_stream(NULL, 1, 1, 1), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_capture_client_close_stream(NULL, 1), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_capture_client_open_stream(file, 0, 1, 1), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_capture_client_open_stream(file, 1, 0, 1), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_capture_client_open_stream(file, 1, 1, 0), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_capture_client_close_stream(file, 0), -EINVAL);
	control = drm_capture_control_file_create(authority);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, control);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, client_stream_put_file, control), 0);
	KUNIT_EXPECT_EQ(test, drm_capture_client_open_stream(control, 1, 1, 1), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_capture_client_close_stream(control, 1), -EINVAL);
	absent = drm_capture_client_file_create(authority, &client_stream_absent_ops, context);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, absent);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, client_stream_put_file, absent), 0);
	KUNIT_EXPECT_EQ(test, drm_capture_client_open_stream(absent, 1, 1, 1), -EOPNOTSUPP);
	KUNIT_EXPECT_EQ(test, drm_capture_client_close_stream(absent, 1), -EOPNOTSUPP);
	KUNIT_EXPECT_EQ(test, context->opens, 0);
	KUNIT_EXPECT_EQ(test, context->closes, 0);
}

static void drm_capture_client_stream_revoke_blocks_open_but_permits_close(struct kunit *test)
{
	struct drm_capture_authority *authority;
	struct client_stream_context *context;
	struct file *file = client_stream_create(test, &authority, &context);

	KUNIT_ASSERT_EQ(test, drm_capture_client_open_stream(file, 1, 19, 2), 0);
	drm_capture_authority_revoke(authority);
	KUNIT_EXPECT_EQ(test, drm_capture_client_open_stream(file, 2, 19, 2), -EKEYREVOKED);
	KUNIT_EXPECT_EQ(test, context->opens, 1);
	KUNIT_ASSERT_EQ(test, drm_capture_client_close_stream(file, 1), 0);
	KUNIT_EXPECT_EQ(test, context->closes, 1);
	KUNIT_EXPECT_FALSE(test, context->active);
}

static void drm_capture_client_stream_requires_a_close_operation(struct kunit *test)
{
	struct drm_capture_authority *authority;
	struct client_stream_context *context;
	struct file *file;

	client_stream_create(test, &authority, &context);
	file = drm_capture_client_file_create(authority, &client_stream_no_close_ops, context);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, file);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, client_stream_put_file, file), 0);
	KUNIT_EXPECT_EQ(test, drm_capture_client_open_stream(file, 1, 19, 2), -EOPNOTSUPP);
	KUNIT_EXPECT_EQ(test, context->opens, 0);
	KUNIT_EXPECT_FALSE(test, context->active);
}

static void drm_capture_client_stream_preserves_provider_errors(struct kunit *test)
{
	struct drm_capture_authority *authority;
	struct client_stream_context *context;
	struct file *file = client_stream_create(test, &authority, &context);

	context->open_result = -EACCES;
	KUNIT_EXPECT_EQ(test, drm_capture_client_open_stream(file, 1, 19, 2), -EACCES);
	context->open_result = 1;
	KUNIT_EXPECT_EQ(test, drm_capture_client_open_stream(file, 1, 19, 2), -EINVAL);
	KUNIT_EXPECT_EQ(test, context->id, 0);
	context->open_result = 0;
	KUNIT_ASSERT_EQ(test, drm_capture_client_open_stream(file, 1, 19, 2), 0);
	context->close_result = -EIO;
	KUNIT_EXPECT_EQ(test, drm_capture_client_close_stream(file, 1), -EIO);
	context->close_result = 1;
	KUNIT_EXPECT_EQ(test, drm_capture_client_close_stream(file, 1), -EINVAL);
	KUNIT_EXPECT_TRUE(test, context->active);
	context->close_result = 0;
	KUNIT_ASSERT_EQ(test, drm_capture_client_close_stream(file, 1), 0);
}

static struct kunit_case drm_capture_client_stream_cases[] = {
	KUNIT_CASE(drm_capture_client_stream_dispatches_named_operations),
	KUNIT_CASE(drm_capture_client_stream_rejects_invalid_endpoints_and_numbers),
	KUNIT_CASE(drm_capture_client_stream_revoke_blocks_open_but_permits_close),
	KUNIT_CASE(drm_capture_client_stream_requires_a_close_operation),
	KUNIT_CASE(drm_capture_client_stream_preserves_provider_errors),
	{}
};

static struct kunit_suite drm_capture_client_stream_suite = {
	.name = "drm_capture_client_stream",
	.test_cases = drm_capture_client_stream_cases,
};

kunit_test_suite(drm_capture_client_stream_suite);
MODULE_LICENSE("GPL");
