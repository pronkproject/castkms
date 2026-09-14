// SPDX-License-Identifier: GPL-2.0-only

#include <linux/err.h>
#include <linux/file.h>
#include <linux/fs.h>
#include <linux/module.h>
#include <linux/poll.h>
#include <drm/drm_capture_authority.h>
#include <drm/drm_capture_file.h>
#include <drm/drm_capture_readiness.h>
#include <kunit/test.h>

struct control_context {
	unsigned int revokes;
	unsigned int client_releases;
	unsigned int readiness_calls;
	struct drm_capture_readiness *readiness;
};

static void client_release(void *data)
{
	struct control_context *context = data;

	context->client_releases++;
}

static const struct drm_capture_client_owner_ops client_ops = {
	.owner = THIS_MODULE,
	.release = client_release,
};

static struct drm_capture_readiness *client_get_readiness(void *data)
{
	struct control_context *context = data;

	context->readiness_calls++;
	return context->readiness ? drm_capture_readiness_get(context->readiness) : NULL;
}

static const struct drm_capture_client_owner_ops client_readiness_ops = {
	.owner = THIS_MODULE,
	.release = client_release,
	.get_readiness = client_get_readiness,
};

static void readiness_put(void *readiness)
{
	drm_capture_readiness_put(readiness);
}

static void control_revoke(void *data)
{
	struct control_context *context = data;

	context->revokes++;
}

static const struct drm_capture_authority_ops control_ops = {
	.owner = THIS_MODULE,
	.revoke = control_revoke,
};

static void control_authority_put(void *authority)
{
	drm_capture_authority_put(authority);
}

static void control_file_put(void *file)
{
	/* KUnit runs in a kernel thread; finish release before its context is freed. */
	__fput_sync(file);
}

static struct file *control_create(struct kunit *test,
				   struct drm_capture_authority **authority,
				   struct control_context **context)
{
	struct file *file;

	*context = kunit_kzalloc(test, sizeof(**context), GFP_KERNEL);
	KUNIT_ASSERT_NOT_NULL(test, *context);
	*authority = drm_capture_authority_create(&control_ops, *context);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, *authority);
	KUNIT_ASSERT_EQ(test,
			kunit_add_action_or_reset(test, control_authority_put, *authority), 0);
	file = drm_capture_control_file_create(*authority);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, file);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, control_file_put, file), 0);
	return file;
}

static void drm_capture_control_last_file_reference(struct kunit *test)
{
	struct drm_capture_authority *authority;
	struct control_context *context;
	struct file *file = control_create(test, &authority, &context);

	get_file(file);
	__fput_sync(file);
	KUNIT_EXPECT_FALSE(test, drm_capture_authority_revoked(authority));
	KUNIT_EXPECT_EQ(test, context->revokes, 0);
	KUNIT_EXPECT_EQ(test, vfs_poll(file, NULL), 0);
	kunit_release_action(test, control_file_put, file);
	KUNIT_EXPECT_TRUE(test, drm_capture_authority_revoked(authority));
	KUNIT_EXPECT_EQ(test, context->revokes, 1);
	/* An ordinary kernel reference survives file close, but not its permission. */
	if (!drm_capture_authority_begin(authority)) {
		drm_capture_authority_end(authority);
		KUNIT_FAIL(test, "closed control file left admission open");
	}
}

static void drm_capture_control_observes_kernel_revoke(struct kunit *test)
{
	struct drm_capture_authority *authority;
	struct control_context *context;
	struct file *file = control_create(test, &authority, &context);
	struct poll_wqueues wait;

	poll_initwait(&wait);
	KUNIT_EXPECT_EQ(test, vfs_poll(file, &wait.pt), 0);
	drm_capture_authority_revoke(authority);
	KUNIT_EXPECT_TRUE(test, wait.triggered);
	KUNIT_EXPECT_EQ(test, vfs_poll(file, NULL), EPOLLHUP);
	KUNIT_EXPECT_EQ(test, vfs_poll(file, NULL), EPOLLHUP);
	poll_freewait(&wait);
	kunit_release_action(test, control_file_put, file);
	KUNIT_EXPECT_EQ(test, context->revokes, 1);
}

static void drm_capture_control_has_no_capture_dispatch(struct kunit *test)
{
	struct drm_capture_authority *authority;
	struct control_context *context;
	struct file *file = control_create(test, &authority, &context);

	KUNIT_EXPECT_PTR_EQ(test, file->f_op->read, NULL);
	KUNIT_EXPECT_PTR_EQ(test, file->f_op->read_iter, NULL);
	KUNIT_EXPECT_PTR_EQ(test, file->f_op->write, NULL);
	KUNIT_EXPECT_PTR_EQ(test, file->f_op->write_iter, NULL);
	KUNIT_EXPECT_PTR_EQ(test, file->f_op->mmap, NULL);
	KUNIT_EXPECT_PTR_EQ(test, file->f_op->llseek, NULL);
	KUNIT_EXPECT_PTR_EQ(test, file->f_op->unlocked_ioctl, NULL);
	KUNIT_EXPECT_PTR_EQ(test, file->f_op->compat_ioctl, NULL);
}

static void drm_capture_control_owner_requires_release(struct kunit *test)
{
	struct drm_capture_authority *authority;
	struct control_context *context;
	const struct drm_capture_control_owner_ops missing_release = { .owner = THIS_MODULE };
	struct file *file;

	control_create(test, &authority, &context);
	file = drm_capture_control_file_create_owned(authority, NULL, context);
	KUNIT_EXPECT_TRUE(test, IS_ERR(file));
	KUNIT_EXPECT_EQ(test, PTR_ERR(file), -EINVAL);
	file = drm_capture_control_file_create_owned(authority, &missing_release, context);
	KUNIT_EXPECT_TRUE(test, IS_ERR(file));
	KUNIT_EXPECT_EQ(test, PTR_ERR(file), -EINVAL);
	KUNIT_EXPECT_FALSE(test, drm_capture_authority_revoked(authority));
	KUNIT_EXPECT_EQ(test, context->revokes, 0);
}

static struct file *client_create(struct kunit *test,
				  struct drm_capture_authority *authority,
				  struct control_context *context)
{
	struct file *file = drm_capture_client_file_create(authority, &client_ops, context);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, file);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, control_file_put, file), 0);
	return file;
}

static void drm_capture_client_close_preserves_sibling_authority(struct kunit *test)
{
	struct drm_capture_authority *authority;
	struct control_context *context;
	struct file *control = control_create(test, &authority, &context);
	struct file *first = client_create(test, authority, context);
	struct file *second = client_create(test, authority, context);
	int ret;

	get_file(first);
	__fput_sync(first);
	KUNIT_EXPECT_EQ(test, context->client_releases, 0);
	kunit_release_action(test, control_file_put, first);
	KUNIT_EXPECT_EQ(test, context->client_releases, 1);
	KUNIT_EXPECT_FALSE(test, drm_capture_authority_revoked(authority));
	ret = drm_capture_authority_begin(authority);
	KUNIT_EXPECT_EQ(test, ret, 0);
	if (!ret)
		drm_capture_authority_end(authority);
	KUNIT_EXPECT_EQ(test, vfs_poll(second, NULL), 0);
	kunit_release_action(test, control_file_put, control);
	KUNIT_EXPECT_EQ(test, context->revokes, 1);
	KUNIT_EXPECT_EQ(test, vfs_poll(second, NULL), EPOLLHUP);
	kunit_release_action(test, control_file_put, second);
	KUNIT_EXPECT_EQ(test, context->client_releases, 2);
	KUNIT_EXPECT_EQ(test, context->revokes, 1);
}

static void drm_capture_client_observes_revocation(struct kunit *test)
{
	struct drm_capture_authority *authority;
	struct control_context *context;
	struct file *file;
	struct poll_wqueues wait;

	control_create(test, &authority, &context);
	file = client_create(test, authority, context);
	poll_initwait(&wait);
	KUNIT_EXPECT_EQ(test, vfs_poll(file, &wait.pt), 0);
	drm_capture_authority_revoke(authority);
	KUNIT_EXPECT_TRUE(test, wait.triggered);
	KUNIT_EXPECT_EQ(test, vfs_poll(file, NULL), EPOLLHUP);
	poll_freewait(&wait);
	KUNIT_EXPECT_EQ(test, context->client_releases, 0);
	kunit_release_action(test, control_file_put, file);
	KUNIT_EXPECT_EQ(test, context->client_releases, 1);
}

static void drm_capture_client_requires_an_owner_release(struct kunit *test)
{
	struct drm_capture_authority *authority;
	struct control_context *context;
	const struct drm_capture_client_owner_ops missing_release = { .owner = THIS_MODULE };
	struct file *file;

	control_create(test, &authority, &context);
	file = drm_capture_client_file_create(authority, NULL, context);
	KUNIT_EXPECT_TRUE(test, IS_ERR(file));
	KUNIT_EXPECT_EQ(test, PTR_ERR(file), -EINVAL);
	file = drm_capture_client_file_create(authority, &missing_release, context);
	KUNIT_EXPECT_TRUE(test, IS_ERR(file));
	KUNIT_EXPECT_EQ(test, PTR_ERR(file), -EINVAL);
	KUNIT_EXPECT_EQ(test, context->client_releases, 0);
	KUNIT_EXPECT_FALSE(test, drm_capture_authority_revoked(authority));
}

static void drm_capture_client_has_no_primary_or_pixel_dispatch(struct kunit *test)
{
	struct drm_capture_authority *authority;
	struct control_context *context;
	struct file *file;

	control_create(test, &authority, &context);
	file = client_create(test, authority, context);
	KUNIT_EXPECT_PTR_EQ(test, file->f_op->read, NULL);
	KUNIT_EXPECT_PTR_EQ(test, file->f_op->read_iter, NULL);
	KUNIT_EXPECT_PTR_EQ(test, file->f_op->write, NULL);
	KUNIT_EXPECT_PTR_EQ(test, file->f_op->write_iter, NULL);
	KUNIT_EXPECT_PTR_EQ(test, file->f_op->mmap, NULL);
	KUNIT_EXPECT_PTR_EQ(test, file->f_op->llseek, NULL);
	KUNIT_ASSERT_NOT_NULL(test, file->f_op->unlocked_ioctl);
	KUNIT_EXPECT_EQ(test, file->f_op->unlocked_ioctl(file, 0, 0), -ENOTTY);
	KUNIT_EXPECT_TRUE(test, file->f_op->compat_ioctl == compat_ptr_ioctl);
}

static void drm_capture_file_pairs_check_roles_and_authority(struct kunit *test)
{
	struct drm_capture_authority *first_authority, *second_authority;
	struct control_context *first_context, *second_context;
	struct file *first_control = control_create(test, &first_authority, &first_context);
	struct file *second_control = control_create(test, &second_authority, &second_context);
	struct file *first = client_create(test, first_authority, first_context);
	struct file *second = client_create(test, second_authority, second_context);

	KUNIT_EXPECT_TRUE(test, drm_capture_files_match(first, first_control));
	KUNIT_EXPECT_TRUE(test, drm_capture_files_match(second, second_control));
	KUNIT_EXPECT_FALSE(test, drm_capture_files_match(first, second_control));
	KUNIT_EXPECT_FALSE(test, drm_capture_files_match(second, first_control));
	KUNIT_EXPECT_FALSE(test, drm_capture_files_match(first_control, first));
	KUNIT_EXPECT_FALSE(test, drm_capture_files_match(first, first));
	KUNIT_EXPECT_FALSE(test, drm_capture_files_match(first_control, first_control));
	KUNIT_EXPECT_FALSE(test, drm_capture_files_match(NULL, first_control));
	KUNIT_EXPECT_FALSE(test, drm_capture_files_match(first, NULL));
	KUNIT_EXPECT_FALSE(test, drm_capture_authority_revoked(first_authority));
	KUNIT_EXPECT_EQ(test, first_context->client_releases, 0);
	drm_capture_authority_revoke(first_authority);
	KUNIT_EXPECT_TRUE(test, drm_capture_files_match(first, first_control));
}

static void drm_capture_client_retains_readiness(struct kunit *test)
{
	struct drm_capture_authority *authority;
	struct control_context *context;
	struct drm_capture_readiness *retained;
	struct file *file;

	control_create(test, &authority, &context);
	context->readiness = drm_capture_readiness_create();
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, context->readiness);
	KUNIT_ASSERT_EQ(test,
			kunit_add_action_or_reset(test, readiness_put, context->readiness), 0);
	file = drm_capture_client_file_create(authority, &client_readiness_ops, context);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, file);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, control_file_put, file), 0);
	KUNIT_EXPECT_EQ(test, context->readiness_calls, 1);
	kunit_release_action(test, readiness_put, context->readiness);
	context->readiness = NULL;
	retained = drm_capture_client_get_readiness(file);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, retained);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, readiness_put, retained), 0);
	KUNIT_EXPECT_FALSE(test, drm_capture_readiness_has_results(retained));
	drm_capture_authority_revoke(authority);
	kunit_release_action(test, control_file_put, file);
	KUNIT_EXPECT_EQ(test, context->client_releases, 1);
	KUNIT_EXPECT_EQ(test, context->readiness_calls, 1);
	drm_capture_readiness_update(retained, true);
	KUNIT_EXPECT_TRUE(test, drm_capture_readiness_has_results(retained));
}

static void drm_capture_client_readiness_checks_role_and_support(struct kunit *test)
{
	struct drm_capture_authority *authority;
	struct control_context *context;
	struct file *control = control_create(test, &authority, &context);
	struct file *client = client_create(test, authority, context);
	struct file *without_notification;

	KUNIT_EXPECT_PTR_EQ(test, drm_capture_client_get_readiness(NULL), ERR_PTR(-EINVAL));
	KUNIT_EXPECT_PTR_EQ(test, drm_capture_client_get_readiness(control), ERR_PTR(-EINVAL));
	KUNIT_EXPECT_PTR_EQ(test, drm_capture_client_get_readiness(client), ERR_PTR(-EOPNOTSUPP));
	without_notification =
		drm_capture_client_file_create(authority, &client_readiness_ops, context);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, without_notification);
	KUNIT_ASSERT_EQ(test,
			kunit_add_action_or_reset(test, control_file_put, without_notification), 0);
	KUNIT_EXPECT_EQ(test, context->readiness_calls, 1);
	KUNIT_EXPECT_PTR_EQ(test, drm_capture_client_get_readiness(without_notification),
			    ERR_PTR(-EOPNOTSUPP));
}

static struct kunit_case drm_capture_file_cases[] = {
	KUNIT_CASE(drm_capture_client_retains_readiness),
	KUNIT_CASE(drm_capture_client_readiness_checks_role_and_support),
	KUNIT_CASE(drm_capture_file_pairs_check_roles_and_authority),
	KUNIT_CASE(drm_capture_client_close_preserves_sibling_authority),
	KUNIT_CASE(drm_capture_client_observes_revocation),
	KUNIT_CASE(drm_capture_client_requires_an_owner_release),
	KUNIT_CASE(drm_capture_client_has_no_primary_or_pixel_dispatch),
	KUNIT_CASE(drm_capture_control_owner_requires_release),
	KUNIT_CASE(drm_capture_control_last_file_reference),
	KUNIT_CASE(drm_capture_control_observes_kernel_revoke),
	KUNIT_CASE(drm_capture_control_has_no_capture_dispatch),
	{}
};

static struct kunit_suite drm_capture_file_suite = {
	.name = "drm_capture_file",
	.test_cases = drm_capture_file_cases,
};

kunit_test_suite(drm_capture_file_suite);
MODULE_LICENSE("GPL");
