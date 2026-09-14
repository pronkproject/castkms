// SPDX-License-Identifier: GPL-2.0-only

#include <linux/err.h>
#include <linux/file.h>
#include <linux/fs.h>
#include <linux/module.h>
#include <drm/drm_capture_authority.h>
#include <drm/drm_capture_file.h>
#include <drm/drm_capture_readiness.h>
#include <kunit/test.h>

struct cancel_context {
	struct drm_capture_readiness *readiness;
	u64 stream;
	u64 use_id;
	unsigned int calls;
	int status;
};

static void cancel_release(void *data)
{
}

static void cancel_revoke(void *data)
{
}

static struct drm_capture_readiness *cancel_get_readiness(void *data)
{
	struct cancel_context *context = data;

	return drm_capture_readiness_get(context->readiness);
}

static int cancel_request(void *data, u64 stream, u64 use_id)
{
	struct cancel_context *context = data;

	context->calls++;
	context->stream = stream;
	context->use_id = use_id;
	return context->status;
}

static const struct drm_capture_authority_ops cancel_authority_ops = {
	.owner = THIS_MODULE,
	.revoke = cancel_revoke,
};

static const struct drm_capture_client_owner_ops cancel_ops = {
	.owner = THIS_MODULE,
	.release = cancel_release,
	.get_readiness = cancel_get_readiness,
	.cancel = cancel_request,
};

static const struct drm_capture_client_owner_ops cancel_absent_ops = {
	.owner = THIS_MODULE,
	.release = cancel_release,
};

static void cancel_put_authority(void *authority)
{
	drm_capture_authority_put(authority);
}

static void cancel_put_file(void *file)
{
	__fput_sync(file);
}

static void cancel_put_readiness(void *readiness)
{
	drm_capture_readiness_put(readiness);
}

static struct file *cancel_create(struct kunit *test,
				  const struct drm_capture_client_owner_ops *ops,
				  struct drm_capture_authority **authority,
				  struct cancel_context **context)
{
	struct file *file;

	*context = kunit_kzalloc(test, sizeof(**context), GFP_KERNEL);
	KUNIT_ASSERT_NOT_NULL(test, *context);
	(*context)->readiness = drm_capture_readiness_create();
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, (*context)->readiness);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, cancel_put_readiness,
							(*context)->readiness), 0);
	*authority = drm_capture_authority_create(&cancel_authority_ops, *context);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, *authority);
	KUNIT_ASSERT_EQ(test,
			kunit_add_action_or_reset(test, cancel_put_authority, *authority), 0);
	file = drm_capture_client_file_create(*authority, ops, *context);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, file);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, cancel_put_file, file), 0);
	return file;
}

static void cancel_forwards_names_without_acknowledging_results(struct kunit *test)
{
	struct drm_capture_authority *authority;
	struct cancel_context *context;
	struct file *file = cancel_create(test, &cancel_ops, &authority, &context);

	drm_capture_readiness_update(context->readiness, true);
	KUNIT_EXPECT_EQ(test, drm_capture_client_cancel(file, 0, 11), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_capture_client_cancel(file, 7, 0), -EINVAL);
	KUNIT_EXPECT_EQ(test, context->calls, 0U);
	KUNIT_EXPECT_EQ(test, drm_capture_client_cancel(file, 7, 11), 0);
	KUNIT_EXPECT_EQ(test, context->calls, 1U);
	KUNIT_EXPECT_EQ(test, context->stream, 7ULL);
	KUNIT_EXPECT_EQ(test, context->use_id, 11ULL);
	KUNIT_EXPECT_TRUE(test, drm_capture_readiness_has_results(context->readiness));
}

static void cancel_remains_available_after_revocation(struct kunit *test)
{
	struct drm_capture_authority *authority;
	struct cancel_context *context;
	struct file *file = cancel_create(test, &cancel_ops, &authority, &context);

	drm_capture_authority_revoke(authority);
	context->status = -EALREADY;
	KUNIT_EXPECT_EQ(test, drm_capture_client_cancel(file, 7, 11), -EALREADY);
	KUNIT_EXPECT_EQ(test, context->calls, 1U);
	context->status = 0;
	KUNIT_EXPECT_EQ(test, drm_capture_client_cancel(file, 7, 12), 0);
	KUNIT_EXPECT_EQ(test, context->calls, 2U);
}

static void cancel_checks_role_support_and_provider_status(struct kunit *test)
{
	struct drm_capture_authority *authority, *other_authority;
	struct cancel_context *context, *other_context;
	struct file *file = cancel_create(test, &cancel_ops, &authority, &context);
	struct file *absent = cancel_create(test, &cancel_absent_ops,
					  &other_authority, &other_context);
	struct file *control = drm_capture_control_file_create(authority);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, control);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, cancel_put_file, control), 0);
	KUNIT_EXPECT_EQ(test, drm_capture_client_cancel(NULL, 7, 11), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_capture_client_cancel(control, 7, 11), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_capture_client_cancel(absent, 7, 11), -EOPNOTSUPP);
	KUNIT_EXPECT_EQ(test, context->calls, 0U);
	context->status = -ENOENT;
	KUNIT_EXPECT_EQ(test, drm_capture_client_cancel(file, 7, 11), -ENOENT);
	context->status = 1;
	KUNIT_EXPECT_EQ(test, drm_capture_client_cancel(file, 7, 11), -EINVAL);
	KUNIT_EXPECT_EQ(test, context->calls, 2U);
}

static struct kunit_case cancel_cases[] = {
	KUNIT_CASE(cancel_forwards_names_without_acknowledging_results),
	KUNIT_CASE(cancel_remains_available_after_revocation),
	KUNIT_CASE(cancel_checks_role_support_and_provider_status),
	{}
};

static struct kunit_suite cancel_suite = {
	.name = "drm_capture_client_cancel",
	.test_cases = cancel_cases,
};

kunit_test_suite(cancel_suite);

MODULE_LICENSE("GPL");
