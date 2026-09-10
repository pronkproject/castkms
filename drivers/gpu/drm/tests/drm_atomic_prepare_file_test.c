// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/err.h>
#include <linux/file.h>
#include <linux/fs.h>
#include <linux/module.h>
#include <linux/poll.h>
#include <drm/drm_atomic_prepare.h>
#include <drm/drm_atomic_prepare_file.h>
#include <drm/drm_atomic_prepare_ticket.h>
#include <kunit/test.h>

struct ticket_file_fixture {
	struct drm_prepare_source *source;
	struct drm_prepare_read_claim *read;
	struct drm_prepare_ticket *ticket;
};

static void free_fixture(void *data)
{
	struct ticket_file_fixture *f = data;

	if (f->read)
		drm_prepare_read_abandon(f->read);
	if (f->ticket)
		drm_prepare_ticket_put(f->ticket);
	drm_prepare_source_put(f->source);
}

static void put_file(void *file)
{
	/* Finish release in the KUnit task before destroying borrowed fixture state. */
	__fput_sync(file);
}

static struct file *new_file(struct kunit *test, struct ticket_file_fixture **fixture,
			     bool pending)
{
	struct ticket_file_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct drm_prepare_retirement_set *set;
	struct drm_prepare_ticket *ticket;
	struct file *file;

	KUNIT_ASSERT_NOT_NULL(test, f);
	f->source = drm_prepare_source_create(1);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->source);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, free_fixture, f), 0);
	if (pending) {
		struct drm_prepare_read_claim *read = drm_prepare_source_claim(f->source);

		KUNIT_ASSERT_NOT_ERR_OR_NULL(test, read);
		f->read = read;
	}
	set = drm_prepare_retirement_set_create(&f->source, 1);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, set);
	ticket = drm_prepare_ticket_create(set);
	drm_prepare_retirement_set_put(set);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, ticket);
	f->ticket = ticket;
	file = drm_prepare_ticket_file_create(ticket);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, file);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_file, file), 0);
	*fixture = f;
	return file;
}

static void final_file_reference_cancels_retained_kernel_ticket(struct kunit *test)
{
	struct ticket_file_fixture *f;
	struct file *file = new_file(test, &f, false);
	struct drm_prepare_ticket *retained = drm_prepare_ticket_file_get_ticket(file);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, retained);
	KUNIT_EXPECT_PTR_EQ(test, retained, f->ticket);
	get_file(file);
	__fput_sync(file);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_ready(retained), 0);
	kunit_release_action(test, put_file, file);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_ready(retained), -ECANCELED);
	drm_prepare_ticket_put(retained);
}

static void poll_observes_release_then_kernel_cancellation(struct kunit *test)
{
	struct ticket_file_fixture *f;
	struct file *file = new_file(test, &f, true);
	struct poll_wqueues wait;

	poll_initwait(&wait);
	KUNIT_EXPECT_EQ(test, vfs_poll(file, &wait.pt), 0);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_status(f->ticket), DRM_PREPARE_TICKET_PENDING);
	drm_prepare_read_release(f->read, NULL);
	f->read = NULL;
	KUNIT_EXPECT_TRUE(test, wait.triggered);
	KUNIT_EXPECT_EQ(test, vfs_poll(file, NULL), EPOLLIN | EPOLLRDNORM);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_status(f->ticket), DRM_PREPARE_TICKET_READY);
	poll_freewait(&wait);
	poll_initwait(&wait);
	KUNIT_EXPECT_EQ(test, vfs_poll(file, &wait.pt), EPOLLIN | EPOLLRDNORM);
	drm_prepare_ticket_cancel(f->ticket);
	KUNIT_EXPECT_TRUE(test, wait.triggered);
	KUNIT_EXPECT_EQ(test, vfs_poll(file, NULL), EPOLLERR | EPOLLHUP);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_status(f->ticket), DRM_PREPARE_TICKET_CANCELED);
	poll_freewait(&wait);
}

static void poll_reports_abandoned_claim_without_readiness(struct kunit *test)
{
	struct ticket_file_fixture *f;
	struct file *file = new_file(test, &f, true);
	struct poll_wqueues wait;

	poll_initwait(&wait);
	KUNIT_EXPECT_EQ(test, vfs_poll(file, &wait.pt), 0);
	drm_prepare_read_abandon(f->read);
	f->read = NULL;
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_status(f->ticket), DRM_PREPARE_TICKET_FAILED);
	KUNIT_EXPECT_TRUE(test, wait.triggered);
	KUNIT_EXPECT_EQ(test, vfs_poll(file, NULL), EPOLLERR | EPOLLHUP);
	poll_freewait(&wait);
}

static int accept(void *data)
{
	return 0;
}

static void closing_consumed_file_preserves_accepted_guard(struct kunit *test)
{
	struct ticket_file_fixture *f;
	struct file *file = new_file(test, &f, false);
	struct drm_prepare_attempt *attempt = drm_prepare_ticket_reserve(f->ticket);
	struct drm_prepare_retirement_guard *guard;
	struct drm_prepare_read_claim *read;
	int ret;

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, attempt);
	ret = drm_prepare_attempt_commit(attempt, accept, NULL, &guard);
	drm_prepare_attempt_destroy(attempt);
	KUNIT_ASSERT_EQ(test, ret, 0);
	KUNIT_EXPECT_EQ(test, vfs_poll(file, NULL), EPOLLHUP);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_status(f->ticket), DRM_PREPARE_TICKET_CONSUMED);
	kunit_release_action(test, put_file, file);
	KUNIT_EXPECT_EQ(test, drm_prepare_ticket_status(f->ticket), DRM_PREPARE_TICKET_CONSUMED);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_source_claim(f->source)), -EBUSY);
	drm_prepare_retirement_guard_destroy(guard);
	read = drm_prepare_source_claim(f->source);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, read);
	drm_prepare_read_release(read, NULL);
}

static void ticket_file_has_no_pixel_or_modeset_dispatch(struct kunit *test)
{
	struct ticket_file_fixture *f;
	struct file *file = new_file(test, &f, false);
	struct file foreign = {};

	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_prepare_ticket_file_get_ticket(&foreign)), -EINVAL);
	KUNIT_EXPECT_PTR_EQ(test, file->f_op->read, NULL);
	KUNIT_EXPECT_PTR_EQ(test, file->f_op->read_iter, NULL);
	KUNIT_EXPECT_PTR_EQ(test, file->f_op->write, NULL);
	KUNIT_EXPECT_PTR_EQ(test, file->f_op->write_iter, NULL);
	KUNIT_EXPECT_PTR_EQ(test, file->f_op->mmap, NULL);
	KUNIT_EXPECT_PTR_EQ(test, file->f_op->llseek, NULL);
	KUNIT_EXPECT_PTR_EQ(test, file->f_op->unlocked_ioctl, NULL);
	KUNIT_EXPECT_PTR_EQ(test, file->f_op->compat_ioctl, NULL);
}

static struct kunit_case cases[] = {
	KUNIT_CASE(final_file_reference_cancels_retained_kernel_ticket),
	KUNIT_CASE(poll_observes_release_then_kernel_cancellation),
	KUNIT_CASE(poll_reports_abandoned_claim_without_readiness),
	KUNIT_CASE(closing_consumed_file_preserves_accepted_guard),
	KUNIT_CASE(ticket_file_has_no_pixel_or_modeset_dispatch),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_prepare_file",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_LICENSE("GPL");
