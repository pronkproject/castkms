// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/err.h>
#include <linux/file.h>
#include <linux/fs.h>
#include <linux/module.h>
#include <linux/mman.h>
#include <linux/poll.h>
#include <linux/uaccess.h>
#include <drm/drm_atomic_prepare.h>
#include <drm/drm_atomic_prepare_outputs.h>
#include <drm/drm_atomic_prepare_file.h>
#include <drm/drm_atomic_prepare_ticket.h>
#include <uapi/drm/drm_prepare.h>
#include <kunit/test.h>

struct ticket_file_fixture {
	struct drm_prepare_source *source;
	struct drm_prepare_read_claim *read;
	struct drm_prepare_ticket *ticket;
	struct drm_prepare_output_generation output;
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
	f->output = (struct drm_prepare_output_generation) { .crtc_id = 1, .source = f->source };
	ticket = drm_prepare_ticket_create(&f->output, 1);
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
	ret = drm_prepare_attempt_commit(attempt, &f->output, 1, accept, NULL, &guard);
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
	KUNIT_ASSERT_NOT_NULL(test, file->f_op->unlocked_ioctl);
	KUNIT_EXPECT_EQ(test, file->f_op->unlocked_ioctl(file, DRM_IOCTL_VERSION, 0), -ENOTTY);
	KUNIT_EXPECT_EQ(test, file->f_op->unlocked_ioctl(file, DRM_IOCTL_PREPARE_QUERY, 0),
			-EFAULT);
	KUNIT_EXPECT_EQ(test, file->f_op->unlocked_ioctl(file,
			_IOWR(DRM_IOCTL_BASE, 0x00, struct drm_prepare_query), 0), -ENOTTY);
	KUNIT_EXPECT_TRUE(test, file->f_op->compat_ioctl == compat_ptr_ioctl);
}

static void expect_query(struct kunit *test, struct file *file, unsigned long address,
			 u32 expected)
{
	struct drm_prepare_query query;
	unsigned int i;

	memset(&query, 0xa5, sizeof(query));
	KUNIT_ASSERT_EQ(test, copy_to_user((void __user *)address, &query, sizeof(query)), 0);
	KUNIT_ASSERT_EQ(test, file->f_op->unlocked_ioctl(file, DRM_IOCTL_PREPARE_QUERY, address),
			0);
	KUNIT_ASSERT_EQ(test, copy_from_user(&query, (void __user *)address, sizeof(query)), 0);
	KUNIT_EXPECT_EQ(test, query.status, expected);
	for (i = 0; i < ARRAY_SIZE(query.reserved); i++)
		KUNIT_EXPECT_EQ(test, query.reserved[i], 0);
}

static void query_observes_status_without_consumption(struct kunit *test)
{
	struct ticket_file_fixture *f;
	struct file *file = new_file(test, &f, true);
	struct drm_prepare_retirement_guard *guard;
	struct drm_prepare_attempt *attempt;
	unsigned long address;
	int ret;

	if (!IS_ENABLED(CONFIG_MMU))
		kunit_skip(test, "userspace query requires MMU");
	address = kunit_vm_mmap(test, NULL, 0, PAGE_SIZE, PROT_READ | PROT_WRITE,
			       MAP_PRIVATE | MAP_ANONYMOUS, 0);
	KUNIT_ASSERT_NE(test, address, 0);
	KUNIT_ASSERT_FALSE(test, IS_ERR_VALUE(address));
	expect_query(test, file, address, DRM_PREPARE_PENDING);
	drm_prepare_read_release(f->read, NULL);
	f->read = NULL;
	expect_query(test, file, address, DRM_PREPARE_READY);
	attempt = drm_prepare_ticket_reserve(f->ticket);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, attempt);
	expect_query(test, file, address, DRM_PREPARE_READY);
	ret = drm_prepare_attempt_commit(attempt, &f->output, 1, accept, NULL, &guard);
	drm_prepare_attempt_destroy(attempt);
	KUNIT_ASSERT_EQ(test, ret, 0);
	drm_prepare_retirement_guard_destroy(guard);
	expect_query(test, file, address, DRM_PREPARE_CONSUMED);

	file = new_file(test, &f, true);
	drm_prepare_read_abandon(f->read);
	f->read = NULL;
	expect_query(test, file, address, DRM_PREPARE_FAILED);
	drm_prepare_ticket_cancel(f->ticket);
	expect_query(test, file, address, DRM_PREPARE_CANCELED);
}

static struct kunit_case cases[] = {
	KUNIT_CASE(final_file_reference_cancels_retained_kernel_ticket),
	KUNIT_CASE(poll_observes_release_then_kernel_cancellation),
	KUNIT_CASE(poll_reports_abandoned_claim_without_readiness),
	KUNIT_CASE(closing_consumed_file_preserves_accepted_guard),
	KUNIT_CASE(ticket_file_has_no_pixel_or_modeset_dispatch),
	KUNIT_CASE(query_observes_status_without_consumption),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_prepare_file",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_LICENSE("GPL");
