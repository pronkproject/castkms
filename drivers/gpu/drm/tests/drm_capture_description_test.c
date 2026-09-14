// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/err.h>
#include <linux/file.h>
#include <linux/module.h>
#include <drm/drm_capture_authority.h>
#include <drm/drm_capture_file.h>
#include <drm/drm_fourcc.h>
#include <kunit/test.h>

struct description_context {
	struct drm_capture_description offered;
	unsigned int calls;
	int result;
};

static void description_release(void *data)
{
}

static void description_revoke(void *data)
{
}

static int description_query(void *data, struct drm_capture_description *result)
{
	struct description_context *context = data;

	context->calls++;
	*result = context->offered;
	return context->result;
}

static const struct drm_capture_authority_ops description_authority_ops = {
	.owner = THIS_MODULE,
	.revoke = description_revoke,
};

static const struct drm_capture_client_owner_ops description_client_ops = {
	.owner = THIS_MODULE,
	.release = description_release,
	.describe = description_query,
};

static const struct drm_capture_client_owner_ops description_absent_ops = {
	.owner = THIS_MODULE,
	.release = description_release,
};

static void description_put_authority(void *authority)
{
	drm_capture_authority_put(authority);
}

static void description_put_file(void *file)
{
	__fput_sync(file);
}

static struct file *description_create(struct kunit *test,
				       struct drm_capture_authority **authority,
				       struct description_context **context)
{
	struct file *file;

	*context = kunit_kzalloc(test, sizeof(**context), GFP_KERNEL);
	KUNIT_ASSERT_NOT_NULL(test, *context);
	(*context)->offered = (struct drm_capture_description) {
		.id = 19,
		.width = 64,
		.height = 32,
		.format = DRM_FORMAT_XRGB8888,
		.max_requests = 8,
		.modifier = DRM_FORMAT_MOD_LINEAR,
	};
	*authority = drm_capture_authority_create(&description_authority_ops, *context);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, *authority);
	KUNIT_ASSERT_EQ(test,
		kunit_add_action_or_reset(test, description_put_authority, *authority), 0);
	file = drm_capture_client_file_create(*authority, &description_client_ops, *context);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, file);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, description_put_file, file), 0);
	return file;
}

static void drm_capture_description_copies_provider_metadata(struct kunit *test)
{
	struct drm_capture_authority *authority;
	struct description_context *context;
	struct file *file = description_create(test, &authority, &context);
	struct drm_capture_description description = {};

	KUNIT_ASSERT_EQ(test, drm_capture_client_describe(file, &description), 0);
	KUNIT_EXPECT_MEMEQ(test, &description, &context->offered, sizeof(description));
	KUNIT_EXPECT_EQ(test, context->calls, 1);
	KUNIT_ASSERT_EQ(test, drm_capture_client_describe(file, &description), 0);
	KUNIT_EXPECT_EQ(test, description.id, 19);
	KUNIT_EXPECT_EQ(test, context->calls, 2);
	KUNIT_EXPECT_FALSE(test, drm_capture_authority_revoked(authority));
}

static void drm_capture_description_errors_preserve_output(struct kunit *test)
{
	struct drm_capture_authority *authority;
	struct description_context *context;
	struct file *file = description_create(test, &authority, &context);
	struct drm_capture_description before = { .id = 71, .width = 999 };
	struct drm_capture_description description = before;
	struct drm_capture_description valid = context->offered;

	context->result = -ESTALE;
	KUNIT_EXPECT_EQ(test, drm_capture_client_describe(file, &description), -ESTALE);
	KUNIT_EXPECT_MEMEQ(test, &description, &before, sizeof(description));
	context->result = 1;
	KUNIT_EXPECT_EQ(test, drm_capture_client_describe(file, &description), -EINVAL);
	KUNIT_EXPECT_MEMEQ(test, &description, &before, sizeof(description));
	context->result = 0;
	for (unsigned int field = 0; field < 6; field++) {
		context->offered = valid;
		switch (field) {
		case 0:
			context->offered.id = 0;
			break;
		case 1:
			context->offered.width = 0;
			break;
		case 2:
			context->offered.height = 0;
			break;
		case 3:
			context->offered.format = 0;
			break;
		case 4:
			context->offered.max_requests = 0;
			break;
		case 5:
			context->offered.modifier = DRM_FORMAT_MOD_INVALID;
			break;
		}
		KUNIT_EXPECT_EQ(test, drm_capture_client_describe(file, &description), -EINVAL);
		KUNIT_EXPECT_MEMEQ(test, &description, &before, sizeof(description));
	}
}

static void drm_capture_description_requires_a_client_provider(struct kunit *test)
{
	struct drm_capture_authority *authority;
	struct description_context *context;
	struct file *client = description_create(test, &authority, &context);
	struct drm_capture_description description = {};
	struct file *control, *absent;

	KUNIT_EXPECT_EQ(test, drm_capture_client_describe(NULL, &description), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_capture_client_describe(client, NULL), -EINVAL);
	control = drm_capture_control_file_create(authority);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, control);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, description_put_file, control), 0);
	KUNIT_EXPECT_EQ(test, drm_capture_client_describe(control, &description), -EINVAL);
	absent = drm_capture_client_file_create(authority, &description_absent_ops, context);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, absent);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, description_put_file, absent), 0);
	KUNIT_EXPECT_EQ(test, drm_capture_client_describe(absent, &description), -EOPNOTSUPP);
	KUNIT_EXPECT_EQ(test, context->calls, 0);
}

static void drm_capture_description_checks_revocation_before_provider(struct kunit *test)
{
	struct drm_capture_authority *authority;
	struct description_context *context;
	struct file *file = description_create(test, &authority, &context);
	struct drm_capture_description description = { .id = 71 };

	drm_capture_authority_revoke(authority);
	KUNIT_EXPECT_EQ(test, drm_capture_client_describe(file, &description), -EKEYREVOKED);
	KUNIT_EXPECT_EQ(test, description.id, 71);
	KUNIT_EXPECT_EQ(test, context->calls, 0);
}

static struct kunit_case drm_capture_description_cases[] = {
	KUNIT_CASE(drm_capture_description_copies_provider_metadata),
	KUNIT_CASE(drm_capture_description_errors_preserve_output),
	KUNIT_CASE(drm_capture_description_requires_a_client_provider),
	KUNIT_CASE(drm_capture_description_checks_revocation_before_provider),
	{}
};

static struct kunit_suite drm_capture_description_suite = {
	.name = "drm_capture_description",
	.test_cases = drm_capture_description_cases,
};

kunit_test_suite(drm_capture_description_suite);
MODULE_LICENSE("GPL");
