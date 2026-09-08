// SPDX-License-Identifier: GPL-2.0

#include <linux/err.h>
#include <linux/module.h>
#include <drm/drm_capture.h>
#include <kunit/test.h>

static void capture_close(void *capture)
{
	drm_capture_close(capture);
}

static struct drm_capture *capture_create(struct kunit *test, unsigned int capacity)
{
	struct drm_capture *capture = drm_capture_create(capacity, 16);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, capture);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, capture_close, capture), 0);
	return capture;
}

static void capture_expect_status(struct kunit *test, struct drm_capture *capture,
				  u64 id, bool completed, int status)
{
	struct drm_capture_result result;

	KUNIT_ASSERT_EQ(test, drm_capture_query(capture, id, &result), 0);
	KUNIT_EXPECT_EQ(test, result.completed, completed);
	KUNIT_EXPECT_EQ(test, result.status, status);
}

static void drm_capture_credits(struct kunit *test)
{
	struct drm_capture *capture = capture_create(test, 1);
	struct drm_capture_job *job;
	u64 first, next = 123;
	u8 pixels[16];

	KUNIT_ASSERT_EQ(test, drm_capture_queue(capture, &first), 0);
	KUNIT_EXPECT_EQ(test, drm_capture_queue(capture, &next), -EAGAIN);
	KUNIT_EXPECT_EQ(test, next, 123);
	capture_expect_status(test, capture, first, false, -EINPROGRESS);
	KUNIT_EXPECT_EQ(test, drm_capture_ack(capture, first), -EBUSY);
	KUNIT_EXPECT_EQ(test, drm_capture_copy_result(capture, first, pixels, sizeof(pixels)),
			-EAGAIN);
	job = drm_capture_claim(capture);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, job);
	KUNIT_EXPECT_EQ(test, drm_capture_job_size(job), sizeof(pixels));
	memset(drm_capture_job_data(job), 0x5a, sizeof(pixels));
	drm_capture_complete(job, 0);
	capture_expect_status(test, capture, first, true, 0);
	KUNIT_EXPECT_EQ(test, drm_capture_queue(capture, &next), -EAGAIN);
	KUNIT_EXPECT_EQ(test, drm_capture_copy_result(capture, first, pixels, 15), -ENOSPC);
	KUNIT_EXPECT_EQ(test, drm_capture_copy_result(capture, first, pixels, sizeof(pixels)),
			sizeof(pixels));
	KUNIT_EXPECT_PTR_EQ(test, memchr_inv(pixels, 0x5a, sizeof(pixels)), NULL);
	KUNIT_ASSERT_EQ(test, drm_capture_ack(capture, first), 0);
	KUNIT_EXPECT_EQ(test, drm_capture_ack(capture, first), -ENOENT);
	KUNIT_ASSERT_EQ(test, drm_capture_queue(capture, &next), 0);
	KUNIT_EXPECT_GT(test, next, first);
	job = drm_capture_claim(capture);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, job);
	KUNIT_EXPECT_PTR_EQ(test, memchr_inv(drm_capture_job_data(job), 0, 16), NULL);
	drm_capture_complete(job, -EIO);
}

static void drm_capture_revoke_claimed(struct kunit *test)
{
	struct drm_capture *capture = capture_create(test, 2);
	struct drm_capture_job *job;
	u64 active, queued;
	u8 pixels[16];

	KUNIT_ASSERT_EQ(test, drm_capture_queue(capture, &active), 0);
	KUNIT_ASSERT_EQ(test, drm_capture_queue(capture, &queued), 0);
	job = drm_capture_claim(capture);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, job);
	drm_capture_revoke(capture);
	drm_capture_revoke(capture);
	capture_expect_status(test, capture, active, false, -EINPROGRESS);
	capture_expect_status(test, capture, queued, true, -EKEYREVOKED);
	KUNIT_EXPECT_EQ(test, drm_capture_ack(capture, active), -EBUSY);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_capture_claim(capture)), -EKEYREVOKED);
	KUNIT_EXPECT_EQ(test, drm_capture_queue(capture, &queued), -EKEYREVOKED);
	/* Authorization of the earlier claim survives revocation. */
	memset(drm_capture_job_data(job), 0xab, 16);
	drm_capture_complete(job, 0);
	capture_expect_status(test, capture, active, true, -EKEYREVOKED);
	memset(pixels, 0, sizeof(pixels));
	KUNIT_EXPECT_EQ(test, drm_capture_copy_result(capture, active, pixels, sizeof(pixels)),
			-EKEYREVOKED);
	KUNIT_EXPECT_PTR_EQ(test, memchr_inv(pixels, 0, sizeof(pixels)), NULL);
}

static void drm_capture_cancel_claimed(struct kunit *test)
{
	struct drm_capture *capture = capture_create(test, 1);
	struct drm_capture_job *job;
	u64 id;

	KUNIT_ASSERT_EQ(test, drm_capture_queue(capture, &id), 0);
	job = drm_capture_claim(capture);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, job);
	KUNIT_EXPECT_EQ(test, drm_capture_cancel(capture, id), 0);
	drm_capture_revoke(capture);
	capture_expect_status(test, capture, id, false, -EINPROGRESS);
	drm_capture_complete(job, -EIO);
	capture_expect_status(test, capture, id, true, -ECANCELED);
	KUNIT_EXPECT_EQ(test, drm_capture_cancel(capture, id), -EALREADY);
}

static void drm_capture_close_claimed(struct kunit *test)
{
	struct drm_capture *capture = capture_create(test, 2);
	struct drm_capture_job *job;
	u64 id;

	KUNIT_ASSERT_EQ(test, drm_capture_queue(capture, &id), 0);
	job = drm_capture_claim(capture);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, job);
	KUNIT_ASSERT_EQ(test, drm_capture_queue(capture, &id), 0);
	kunit_release_action(test, capture_close, capture);
	/* No caller reference remains, but the provider still owns its job. */
	memset(drm_capture_job_data(job), 0xff, drm_capture_job_size(job));
	drm_capture_complete(job, 0);
}

static void drm_capture_cancel_queued(struct kunit *test)
{
	struct drm_capture *capture = capture_create(test, 1);
	u64 id;

	KUNIT_ASSERT_EQ(test, drm_capture_queue(capture, &id), 0);
	KUNIT_EXPECT_EQ(test, drm_capture_cancel(capture, id), 0);
	capture_expect_status(test, capture, id, true, -ECANCELED);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_capture_claim(capture)), -EAGAIN);
	KUNIT_EXPECT_EQ(test, drm_capture_ack(capture, id), 0);
}

static struct kunit_case drm_capture_cases[] = {
	KUNIT_CASE(drm_capture_credits),
	KUNIT_CASE(drm_capture_revoke_claimed),
	KUNIT_CASE(drm_capture_cancel_claimed),
	KUNIT_CASE(drm_capture_close_claimed),
	KUNIT_CASE(drm_capture_cancel_queued),
	{}
};

static struct kunit_suite drm_capture_suite = {
	.name = "drm_capture",
	.test_cases = drm_capture_cases,
};

kunit_test_suite(drm_capture_suite);

MODULE_LICENSE("GPL");
