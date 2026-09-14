// SPDX-License-Identifier: GPL-2.0-only

#include <linux/err.h>
#include <linux/file.h>
#include <linux/module.h>
#include <drm/drm_capture_authority.h>
#include <drm/drm_capture_completion.h>
#include <drm/drm_capture_file.h>
#include <kunit/test.h>

enum completion_behavior {
	COMPLETION_NORMAL,
	COMPLETION_OMIT,
	COMPLETION_REPEAT,
	COMPLETION_IGNORE_ERROR,
};

struct completion_context {
	struct drm_capture_completion result;
	enum completion_behavior behavior;
	bool ready;
	unsigned int calls;
};

struct publication_context {
	struct drm_capture_completion received;
	unsigned int calls;
	int status;
};

static void completion_release(void *data)
{
}

static void completion_revoke(void *data)
{
}

static int completion_dequeue(void *data, u64 stream,
			      const struct drm_capture_completion_sink *sink)
{
	struct completion_context *context = data;
	int ret;

	context->calls++;
	if (stream != 7)
		return -ENOENT;
	if (!context->ready)
		return -EAGAIN;
	if (context->behavior == COMPLETION_OMIT)
		return 0;
	ret = sink->publish(sink->data, &context->result);
	if (context->behavior == COMPLETION_REPEAT)
		return sink->publish(sink->data, &context->result);
	if (context->behavior == COMPLETION_IGNORE_ERROR)
		return 0;
	if (!ret)
		context->ready = false;
	return ret;
}

static int completion_publish(void *data, const struct drm_capture_completion *completion)
{
	struct publication_context *publication = data;

	publication->calls++;
	publication->received = *completion;
	return publication->status;
}

static const struct drm_capture_authority_ops completion_authority_ops = {
	.owner = THIS_MODULE,
	.revoke = completion_revoke,
};

static const struct drm_capture_client_owner_ops completion_client_ops = {
	.owner = THIS_MODULE,
	.release = completion_release,
	.dequeue = completion_dequeue,
};

static const struct drm_capture_client_owner_ops completion_absent_ops = {
	.owner = THIS_MODULE,
	.release = completion_release,
};

static void completion_put_authority(void *authority)
{
	drm_capture_authority_put(authority);
}

static void completion_put_file(void *file)
{
	__fput_sync(file);
}

static struct file *completion_create(struct kunit *test,
				      struct drm_capture_authority **authority,
				      struct completion_context **context)
{
	struct file *file;

	*context = kunit_kzalloc(test, sizeof(**context), GFP_KERNEL);
	KUNIT_ASSERT_NOT_NULL(test, *context);
	(*context)->ready = true;
	(*context)->result.use_id = 19;
	(*context)->result.completed_at = 123456;
	*authority = drm_capture_authority_create(&completion_authority_ops, *context);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, *authority);
	KUNIT_ASSERT_EQ(test,
			kunit_add_action_or_reset(test, completion_put_authority, *authority), 0);
	file = drm_capture_client_file_create(*authority, &completion_client_ops, *context);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, file);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, completion_put_file, file), 0);
	return file;
}

static void drm_capture_client_completion_acknowledges_after_publication(struct kunit *test)
{
	struct drm_capture_authority *authority;
	struct completion_context *context;
	struct file *file = completion_create(test, &authority, &context);
	struct publication_context publication = {};
	struct drm_capture_completion_sink sink = { completion_publish, &publication };

	KUNIT_EXPECT_EQ(test, drm_capture_client_dequeue(file, 7, &sink), 0);
	KUNIT_EXPECT_EQ(test, publication.calls, 1);
	KUNIT_EXPECT_EQ(test, publication.received.use_id, 19);
	KUNIT_EXPECT_EQ(test, publication.received.status, 0);
	KUNIT_EXPECT_EQ(test, publication.received.completed_at, 123456);
	KUNIT_EXPECT_FALSE(test, context->ready);
	KUNIT_EXPECT_EQ(test, drm_capture_client_dequeue(file, 7, &sink), -EAGAIN);
	KUNIT_EXPECT_EQ(test, publication.calls, 1);
}

static void drm_capture_client_completion_retries_after_fault_and_revocation(struct kunit *test)
{
	struct drm_capture_authority *authority;
	struct completion_context *context;
	struct file *file = completion_create(test, &authority, &context);
	struct publication_context publication = { .status = -EFAULT };
	struct drm_capture_completion_sink sink = { completion_publish, &publication };

	KUNIT_EXPECT_EQ(test, drm_capture_client_dequeue(file, 7, &sink), -EFAULT);
	KUNIT_EXPECT_TRUE(test, context->ready);
	drm_capture_authority_revoke(authority);
	publication.status = 0;
	KUNIT_EXPECT_EQ(test, drm_capture_client_dequeue(file, 7, &sink), 0);
	KUNIT_EXPECT_EQ(test, publication.calls, 2);
	KUNIT_EXPECT_EQ(test, publication.received.use_id, 19);
	KUNIT_EXPECT_EQ(test, publication.received.completed_at, 123456);
	KUNIT_EXPECT_FALSE(test, context->ready);
}

static void drm_capture_client_completion_validates_terminal_metadata(struct kunit *test)
{
	struct drm_capture_authority *authority;
	struct completion_context *context;
	struct file *file = completion_create(test, &authority, &context);
	struct publication_context publication = {};
	struct drm_capture_completion_sink sink = { completion_publish, &publication };
	const struct drm_capture_completion invalid[] = {
		{ .use_id = 0 },
		{ .use_id = 19, .status = 1 },
		{ .use_id = 19, .status = -4096 },
		{ .use_id = 19, .completed_at = -1 },
		{ .use_id = 19, .status = -EIO, .completed_at = 1 },
	};
	unsigned int i;

	for (i = 0; i < ARRAY_SIZE(invalid); i++) {
		context->result = invalid[i];
		KUNIT_EXPECT_EQ(test, drm_capture_client_dequeue(file, 7, &sink), -EINVAL);
		KUNIT_EXPECT_TRUE(test, context->ready);
	}
	KUNIT_EXPECT_EQ(test, publication.calls, 0);
	context->result = (struct drm_capture_completion) { .use_id = 19, .status = -EIO };
	KUNIT_EXPECT_EQ(test, drm_capture_client_dequeue(file, 7, &sink), 0);
	KUNIT_EXPECT_EQ(test, publication.received.status, -EIO);
	KUNIT_EXPECT_EQ(test, publication.received.completed_at, 0);
	KUNIT_EXPECT_FALSE(test, context->ready);
}

static void drm_capture_client_completion_checks_role_arguments_and_support(struct kunit *test)
{
	struct drm_capture_authority *authority;
	struct completion_context *context;
	struct file *file = completion_create(test, &authority, &context);
	struct publication_context publication = {};
	struct drm_capture_completion_sink sink = { completion_publish, &publication };
	struct drm_capture_completion_sink invalid = {};
	struct file *control, *absent;

	control = drm_capture_control_file_create(authority);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, control);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, completion_put_file, control), 0);
	absent = drm_capture_client_file_create(authority, &completion_absent_ops, context);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, absent);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, completion_put_file, absent), 0);
	KUNIT_EXPECT_EQ(test, drm_capture_client_dequeue(NULL, 7, &sink), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_capture_client_dequeue(control, 7, &sink), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_capture_client_dequeue(file, 0, &sink), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_capture_client_dequeue(file, 7, NULL), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_capture_client_dequeue(file, 7, &invalid), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_capture_client_dequeue(absent, 7, &sink), -EOPNOTSUPP);
	KUNIT_EXPECT_EQ(test, context->calls, 0);
	KUNIT_EXPECT_EQ(test, drm_capture_client_dequeue(file, 8, &sink), -ENOENT);
	KUNIT_EXPECT_EQ(test, publication.calls, 0);
}

static void drm_capture_client_completion_rejects_broken_publication_contracts(struct kunit *test)
{
	struct drm_capture_authority *authority;
	struct completion_context *context;
	struct file *file = completion_create(test, &authority, &context);
	struct publication_context publication = {};
	struct drm_capture_completion_sink sink = { completion_publish, &publication };

	context->behavior = COMPLETION_OMIT;
	KUNIT_EXPECT_EQ(test, drm_capture_client_dequeue(file, 7, &sink), -EIO);
	KUNIT_EXPECT_EQ(test, publication.calls, 0);
	context->behavior = COMPLETION_REPEAT;
	KUNIT_EXPECT_EQ(test, drm_capture_client_dequeue(file, 7, &sink), -EIO);
	KUNIT_EXPECT_EQ(test, publication.calls, 1);
	context->behavior = COMPLETION_IGNORE_ERROR;
	publication.status = -EFAULT;
	KUNIT_EXPECT_EQ(test, drm_capture_client_dequeue(file, 7, &sink), -EIO);
	KUNIT_EXPECT_EQ(test, publication.calls, 2);
}

static struct kunit_case drm_capture_client_completion_cases[] = {
	KUNIT_CASE(drm_capture_client_completion_acknowledges_after_publication),
	KUNIT_CASE(drm_capture_client_completion_retries_after_fault_and_revocation),
	KUNIT_CASE(drm_capture_client_completion_validates_terminal_metadata),
	KUNIT_CASE(drm_capture_client_completion_checks_role_arguments_and_support),
	KUNIT_CASE(drm_capture_client_completion_rejects_broken_publication_contracts),
	{}
};

static struct kunit_suite drm_capture_client_completion_suite = {
	.name = "drm_capture_client_completion",
	.test_cases = drm_capture_client_completion_cases,
};

kunit_test_suite(drm_capture_client_completion_suite);
MODULE_LICENSE("GPL");
