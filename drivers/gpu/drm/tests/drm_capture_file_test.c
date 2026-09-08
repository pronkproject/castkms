// SPDX-License-Identifier: GPL-2.0-only

#include <linux/err.h>
#include <linux/file.h>
#include <linux/fs.h>
#include <linux/module.h>
#include <linux/poll.h>
#include <drm/drm_capture_authority.h>
#include <drm/drm_capture_file.h>
#include <kunit/test.h>

struct control_context {
	unsigned int revokes;
};

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

static struct kunit_case drm_capture_file_cases[] = {
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
