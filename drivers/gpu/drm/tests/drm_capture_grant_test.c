// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/err.h>
#include <linux/file.h>
#include <linux/fs.h>
#include <linux/module.h>
#include <drm/drm_capture_authority.h>
#include <drm/drm_capture_file.h>
#include <drm/drm_capture_grant.h>
#include <drm/drm_device.h>
#include <drm/drm_drv.h>
#include <drm/drm_file.h>
#include <kunit/test.h>

/* Dispatch-only objects: no registration, modeset or real pixel authorization. */
struct grant_context {
	struct drm_device dev;
	struct drm_minor minor;
	struct drm_file file;
	struct drm_capture_authority *authority;
	struct file *capture;
	struct file *control;
	struct drm_capture_target observed;
	unsigned int calls;
	int status;
	bool swapped;
	bool partial;
	bool error_pointer;
};

static void grant_noop(void *data) {}

static const struct drm_capture_authority_ops authority_ops = {
	.owner = THIS_MODULE,
	.revoke = grant_noop,
};

static const struct drm_capture_client_owner_ops client_ops = {
	.owner = THIS_MODULE,
	.release = grant_noop,
};

static void grant_authority_put(void *data)
{
	drm_capture_authority_put(data);
}

static void grant_file_put(void *data)
{
	/* Finish file cleanup before KUnit frees the fixture backing its authority. */
	__fput_sync(data);
}

static int grant_create(struct drm_device *dev, struct drm_file *file,
			const struct drm_capture_target *target, struct drm_capture_files *files)
{
	struct grant_context *context = dev->dev_private;

	context->calls++;
	context->observed = *target;
	files->capture = context->error_pointer ? ERR_PTR(-ENOMEM) :
		get_file(context->swapped ? context->control : context->capture);
	if (!context->partial)
		files->control = get_file(context->swapped ? context->capture : context->control);
	return context->status;
}

static const struct drm_mode_config_funcs grant_funcs = {
	.create_capture_grant = grant_create,
};

static const struct drm_driver grant_driver = {
	.driver_features = DRIVER_MODESET,
};

static struct grant_context *grant_fixture(struct kunit *test)
{
	struct grant_context *context = kunit_kzalloc(test, sizeof(*context), GFP_KERNEL);

	KUNIT_ASSERT_NOT_NULL(test, context);
	context->dev.driver = &grant_driver;
	context->dev.registered = true;
	context->dev.driver_features = DRIVER_MODESET;
	context->dev.mode_config.funcs = &grant_funcs;
	context->dev.dev_private = context;
	context->minor.dev = &context->dev;
	context->file.minor = &context->minor;
	context->authority = drm_capture_authority_create(&authority_ops, context);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, context->authority);
	KUNIT_ASSERT_EQ(test,
		kunit_add_action_or_reset(test, grant_authority_put, context->authority), 0);
	context->capture = drm_capture_client_file_create(context->authority, &client_ops, context);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, context->capture);
	KUNIT_ASSERT_EQ(test,
		kunit_add_action_or_reset(test, grant_file_put, context->capture), 0);
	context->control = drm_capture_control_file_create(context->authority);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, context->control);
	KUNIT_ASSERT_EQ(test,
		kunit_add_action_or_reset(test, grant_file_put, context->control), 0);
	return context;
}

static void drm_capture_grant_validates_target_and_participation(struct kunit *test)
{
	struct grant_context *context = grant_fixture(test);
	struct drm_capture_target target = { .crtc_id = 7, .connector_id = 9 };
	struct drm_capture_files result = {};

	context->dev.mode_config.funcs = NULL;
	KUNIT_EXPECT_EQ(test, drm_capture_create_file_grant(&context->dev, &context->file,
		&target, &result), -EOPNOTSUPP);
	context->dev.mode_config.funcs = &grant_funcs;
	context->dev.driver_features = 0;
	KUNIT_EXPECT_EQ(test, drm_capture_create_file_grant(&context->dev, &context->file,
		&target, &result), -EOPNOTSUPP);
	context->dev.driver_features = DRIVER_MODESET;
	context->dev.unplugged = true;
	KUNIT_EXPECT_EQ(test, drm_capture_create_file_grant(&context->dev, &context->file,
		&target, &result), -ENODEV);
	context->dev.unplugged = false;
	context->dev.registered = false;
	KUNIT_EXPECT_EQ(test, drm_capture_create_file_grant(&context->dev, &context->file,
		&target, &result), -ENODEV);
	context->dev.registered = true;
	context->minor.dev = NULL;
	KUNIT_EXPECT_EQ(test, drm_capture_create_file_grant(&context->dev, &context->file,
		&target, &result), -EINVAL);
	context->minor.dev = &context->dev;
	target.connector_id = 0;
	KUNIT_EXPECT_EQ(test, drm_capture_create_file_grant(&context->dev, &context->file,
		&target, &result), -EINVAL);
	KUNIT_EXPECT_EQ(test, context->calls, 0);
	KUNIT_EXPECT_PTR_EQ(test, result.capture, NULL);
	KUNIT_EXPECT_PTR_EQ(test, result.control, NULL);
}

static void drm_capture_grant_transfers_matching_owned_endpoints(struct kunit *test)
{
	struct grant_context *context = grant_fixture(test);
	struct drm_capture_target target = { .crtc_id = 7, .connector_id = 9 };
	struct drm_capture_files result = {};

	KUNIT_ASSERT_EQ(test, drm_capture_create_file_grant(&context->dev, &context->file,
		&target, &result), 0);
	KUNIT_EXPECT_EQ(test, context->calls, 1);
	KUNIT_EXPECT_EQ(test, context->observed.crtc_id, 7);
	KUNIT_EXPECT_EQ(test, context->observed.connector_id, 9);
	KUNIT_EXPECT_PTR_EQ(test, result.capture, context->capture);
	KUNIT_EXPECT_PTR_EQ(test, result.control, context->control);
	KUNIT_EXPECT_EQ(test, file_count(context->capture), 2);
	KUNIT_EXPECT_EQ(test, file_count(context->control), 2);
	drm_capture_files_put(&result);
	KUNIT_EXPECT_PTR_EQ(test, result.capture, NULL);
	KUNIT_EXPECT_PTR_EQ(test, result.control, NULL);
	KUNIT_EXPECT_EQ(test, file_count(context->capture), 1);
	KUNIT_EXPECT_EQ(test, file_count(context->control), 1);
	KUNIT_EXPECT_FALSE(test, drm_capture_authority_revoked(context->authority));
}

static void drm_capture_grant_rejects_invalid_provider_replies(struct kunit *test)
{
	struct grant_context *context = grant_fixture(test);
	struct drm_capture_target target = { .crtc_id = 7, .connector_id = 9 };
	struct drm_capture_files result = { .capture = ERR_PTR(-EBUSY) };

	context->swapped = true;
	KUNIT_EXPECT_EQ(test, drm_capture_create_file_grant(&context->dev, &context->file,
		&target, &result), -EINVAL);
	context->swapped = false;
	context->status = 1;
	KUNIT_EXPECT_EQ(test, drm_capture_create_file_grant(&context->dev, &context->file,
		&target, &result), -EINVAL);
	context->status = 0;
	context->error_pointer = true;
	KUNIT_EXPECT_EQ(test, drm_capture_create_file_grant(&context->dev, &context->file,
		&target, &result), -EINVAL);
	KUNIT_EXPECT_PTR_EQ(test, result.capture, ERR_PTR(-EBUSY));
	KUNIT_EXPECT_PTR_EQ(test, result.control, NULL);
	KUNIT_EXPECT_EQ(test, file_count(context->capture), 1);
	KUNIT_EXPECT_EQ(test, file_count(context->control), 1);
	KUNIT_EXPECT_FALSE(test, drm_capture_authority_revoked(context->authority));
}

static void drm_capture_grant_releases_partial_error_results(struct kunit *test)
{
	struct grant_context *context = grant_fixture(test);
	struct drm_capture_target target = { .crtc_id = 7, .connector_id = 9 };
	struct drm_capture_files result = { .capture = ERR_PTR(-EBUSY) };

	context->partial = true;
	context->status = -EACCES;
	KUNIT_EXPECT_EQ(test, drm_capture_create_file_grant(&context->dev, &context->file,
		&target, &result), -EACCES);
	KUNIT_EXPECT_EQ(test, file_count(context->capture), 1);
	KUNIT_EXPECT_EQ(test, file_count(context->control), 1);
	KUNIT_EXPECT_PTR_EQ(test, result.capture, ERR_PTR(-EBUSY));
	KUNIT_EXPECT_PTR_EQ(test, result.control, NULL);
}

static struct kunit_case drm_capture_grant_cases[] = {
	KUNIT_CASE(drm_capture_grant_validates_target_and_participation),
	KUNIT_CASE(drm_capture_grant_transfers_matching_owned_endpoints),
	KUNIT_CASE(drm_capture_grant_rejects_invalid_provider_replies),
	KUNIT_CASE(drm_capture_grant_releases_partial_error_results),
	{}
};

static struct kunit_suite drm_capture_grant_suite = {
	.name = "drm_capture_grant",
	.test_cases = drm_capture_grant_cases,
};

kunit_test_suite(drm_capture_grant_suite);
MODULE_LICENSE("GPL");
