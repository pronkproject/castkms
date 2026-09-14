// SPDX-License-Identifier: GPL-2.0

#include <linux/err.h>
#include <linux/limits.h>
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

static void drm_capture_independent_owners(struct kunit *test)
{
	struct drm_capture *capture = capture_create(test, 1);
	struct drm_capture *provider = drm_capture_get(capture);
	struct drm_capture_job *job;
	u64 id;

	KUNIT_ASSERT_EQ(test, drm_capture_queue(capture, &id), 0);
	job = drm_capture_claim(provider);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, job);
	kunit_release_action(test, capture_close, capture);
	/* The provider's reference permits ordinary calls after endpoint close. */
	KUNIT_EXPECT_EQ(test, drm_capture_queue(provider, &id), -EKEYREVOKED);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_capture_claim(provider)), -EKEYREVOKED);
	drm_capture_shutdown(provider);
	drm_capture_shutdown(provider);
	memset(drm_capture_job_data(job), 0x33, drm_capture_job_size(job));
	drm_capture_complete(job, 0);
	drm_capture_put(provider);
}

static void drm_capture_put_preserves_other_owners(struct kunit *test)
{
	struct drm_capture *capture = capture_create(test, 1);
	struct drm_capture *observer = drm_capture_get(capture);
	u8 image[16] = {};
	u64 id;

	drm_capture_put(observer);
	KUNIT_ASSERT_EQ(test, drm_capture_queue(capture, &id), 0);
	KUNIT_EXPECT_EQ(test, drm_capture_publish_snapshot(capture, image, sizeof(image)), 0);
	capture_expect_status(test, capture, id, true, 0);
}

static void drm_capture_last_put_drains_requests(struct kunit *test)
{
	struct drm_capture *capture = capture_create(test, 2);
	struct drm_capture_job *job;
	u64 id;

	KUNIT_ASSERT_EQ(test, drm_capture_queue(capture, &id), 0);
	job = drm_capture_claim(capture);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, job);
	KUNIT_EXPECT_EQ(test, drm_capture_queue(capture, &id), 0);
	kunit_remove_action(test, capture_close, capture);
	drm_capture_put(capture);
	/* Completion drops the last reference, including the unclaimed request. */
	drm_capture_complete(job, -EIO);
}

static void drm_capture_snapshot_isolation(struct kunit *test)
{
	struct drm_capture *old = capture_create(test, 2);
	struct drm_capture *fresh = capture_create(test, 1);
	u8 image[16], retained[16], result[16];
	u64 first, second, next;

	memset(image, 0x11, sizeof(image));
	KUNIT_EXPECT_EQ(test, drm_capture_publish_snapshot(old, image, sizeof(image)), -EAGAIN);
	KUNIT_ASSERT_EQ(test, drm_capture_queue(old, &first), 0);
	KUNIT_ASSERT_EQ(test, drm_capture_queue(old, &second), 0);
	KUNIT_EXPECT_EQ(test, drm_capture_publish_snapshot(old, image, 15), -EINVAL);
	capture_expect_status(test, old, first, false, -EINPROGRESS);
	KUNIT_ASSERT_EQ(test, drm_capture_publish_snapshot(old, image, sizeof(image)), 0);
	memset(image, 0x22, sizeof(image));
	KUNIT_ASSERT_EQ(test, drm_capture_publish_snapshot(old, image, sizeof(image)), 0);
	memset(image, 0xff, sizeof(image));
	KUNIT_ASSERT_EQ(test, drm_capture_copy_result(old, first, retained, sizeof(retained)), 16);
	KUNIT_EXPECT_PTR_EQ(test, memchr_inv(retained, 0x11, sizeof(retained)), NULL);
	KUNIT_ASSERT_EQ(test, drm_capture_copy_result(old, second, result, sizeof(result)), 16);
	KUNIT_EXPECT_PTR_EQ(test, memchr_inv(result, 0x22, sizeof(result)), NULL);
	drm_capture_revoke(old);
	KUNIT_ASSERT_EQ(test, drm_capture_queue(fresh, &next), 0);
	KUNIT_ASSERT_EQ(test, drm_capture_publish_snapshot(fresh, image, sizeof(image)), 0);
	KUNIT_ASSERT_EQ(test, drm_capture_copy_result(fresh, next, result, sizeof(result)), 16);
	KUNIT_EXPECT_PTR_EQ(test, memchr_inv(result, 0xff, sizeof(result)), NULL);
	/* Old completed results are immutable and never acquire new-scope bytes. */
	KUNIT_ASSERT_EQ(test, drm_capture_copy_result(old, first, result, sizeof(result)), 16);
	KUNIT_EXPECT_MEMEQ(test, retained, result, sizeof(result));
	KUNIT_EXPECT_EQ(test, drm_capture_publish_snapshot(old, image, sizeof(image)),
			-EKEYREVOKED);
}

static void drm_capture_failed_image(struct kunit *test)
{
	struct drm_capture *capture = capture_create(test, 1);
	struct drm_capture_job *job;
	u8 result[16] = {};
	u64 id;

	KUNIT_ASSERT_EQ(test, drm_capture_queue(capture, &id), 0);
	job = drm_capture_claim(capture);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, job);
	memset(drm_capture_job_data(job), 0xff, 16);
	drm_capture_complete(job, -EIO);
	capture_expect_status(test, capture, id, true, -EIO);
	KUNIT_EXPECT_EQ(test, drm_capture_copy_result(capture, id, result, sizeof(result)), -EIO);
	KUNIT_EXPECT_PTR_EQ(test, memchr_inv(result, 0, sizeof(result)), NULL);
	KUNIT_EXPECT_EQ(test, drm_capture_ack(capture, id), 0);
}

static void capture_complete_cancel(void *job)
{
	drm_capture_complete(job, -ECANCELED);
}

static void drm_capture_discard_unclaimed(struct kunit *test)
{
	struct drm_capture *capture = capture_create(test, 1);
	struct drm_capture_result result;
	u8 pixels[16] = {};
	u64 id, next;

	KUNIT_ASSERT_EQ(test, drm_capture_queue(capture, &id), 0);
	KUNIT_EXPECT_EQ(test, drm_capture_discard(capture, id), 0);
	KUNIT_EXPECT_EQ(test, drm_capture_query(capture, id, &result), -ENOENT);
	KUNIT_EXPECT_EQ(test, drm_capture_discard(capture, id), -ENOENT);
	KUNIT_ASSERT_EQ(test, drm_capture_queue(capture, &next), 0);
	KUNIT_EXPECT_NE(test, id, next);
	KUNIT_ASSERT_EQ(test, drm_capture_publish_snapshot(capture, pixels, sizeof(pixels)), 0);
	KUNIT_EXPECT_EQ(test, drm_capture_discard(capture, next), 0);
	KUNIT_EXPECT_EQ(test, drm_capture_copy_result(capture, next, pixels, sizeof(pixels)),
			-ENOENT);
	KUNIT_EXPECT_EQ(test, drm_capture_queue(capture, &next), 0);
}

static void drm_capture_discard_claimed(struct kunit *test)
{
	struct drm_capture *capture = capture_create(test, 1);
	struct drm_capture_job *job;
	struct drm_capture_result result;
	u64 id, next;

	KUNIT_ASSERT_EQ(test, drm_capture_queue(capture, &id), 0);
	job = drm_capture_claim(capture);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, job);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, capture_complete_cancel, job), 0);
	KUNIT_EXPECT_EQ(test, drm_capture_discard(capture, id), 0);
	KUNIT_EXPECT_EQ(test, drm_capture_query(capture, id, &result), -ENOENT);
	KUNIT_EXPECT_EQ(test, drm_capture_cancel(capture, id), -ENOENT);
	KUNIT_EXPECT_EQ(test, drm_capture_ack(capture, id), -ENOENT);
	KUNIT_EXPECT_EQ(test, drm_capture_queue(capture, &next), -EAGAIN);
	/* Discarded work still owns writable storage and its bounded credit. */
	memset(drm_capture_job_data(job), 0x5a, drm_capture_job_size(job));
	kunit_remove_action(test, capture_complete_cancel, job);
	drm_capture_complete(job, 0);
	KUNIT_EXPECT_EQ(test, drm_capture_query(capture, id, &result), -ENOENT);
	KUNIT_EXPECT_EQ(test, drm_capture_queue(capture, &next), 0);
}

static void drm_capture_discard_survives_close(struct kunit *test)
{
	struct drm_capture *capture = capture_create(test, 1);
	struct drm_capture_job *job;
	u64 id;

	KUNIT_ASSERT_EQ(test, drm_capture_queue(capture, &id), 0);
	job = drm_capture_claim(capture);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, job);
	KUNIT_EXPECT_EQ(test, drm_capture_discard(capture, id), 0);
	drm_capture_revoke(capture);
	kunit_remove_action(test, capture_close, capture);
	drm_capture_close(capture);
	memset(drm_capture_job_data(job), 0x5a, drm_capture_job_size(job));
	drm_capture_complete(job, -EIO);
}

static void drm_capture_result_ranges(struct kunit *test)
{
	struct drm_capture *capture = capture_create(test, 1);
	u8 source[16], pixels[4];
	u64 id, next;
	unsigned int i;

	for (i = 0; i < sizeof(source); i++)
		source[i] = i;
	KUNIT_ASSERT_EQ(test, drm_capture_queue(capture, &id), 0);
	KUNIT_ASSERT_EQ(test, drm_capture_publish_snapshot(capture, source, sizeof(source)), 0);
	KUNIT_EXPECT_EQ(test, drm_capture_copy_result_range(capture, id, 5, pixels, 4), 4);
	KUNIT_EXPECT_MEMEQ(test, pixels, source + 5, 4);
	KUNIT_EXPECT_EQ(test, drm_capture_copy_result_range(capture, id, 12, pixels, 4), 4);
	KUNIT_EXPECT_MEMEQ(test, pixels, source + 12, 4);
	KUNIT_EXPECT_EQ(test, drm_capture_copy_result_range(capture, id, 16, NULL, 0), 0);
	KUNIT_EXPECT_EQ(test, drm_capture_queue(capture, &next), -EAGAIN);
	drm_capture_revoke(capture);
	KUNIT_EXPECT_EQ(test, drm_capture_copy_result_range(capture, id, 0, pixels, 4), 4);
	KUNIT_EXPECT_MEMEQ(test, pixels, source, 4);
	KUNIT_EXPECT_EQ(test, drm_capture_ack(capture, id), 0);
	KUNIT_EXPECT_EQ(test, drm_capture_copy_result_range(capture, id, 0, pixels, 4), -ENOENT);
}

static void drm_capture_rejected_ranges_preserve_output(struct kunit *test)
{
	struct drm_capture *capture = capture_create(test, 1);
	u8 source[16] = { 0 }, pixels[4];
	u64 id;

	memset(pixels, 0xa9, sizeof(pixels));
	KUNIT_ASSERT_EQ(test, drm_capture_queue(capture, &id), 0);
	KUNIT_ASSERT_EQ(test, drm_capture_publish_snapshot(capture, source, sizeof(source)), 0);
	KUNIT_EXPECT_EQ(test, drm_capture_copy_result_range(capture, id, 13, pixels, 4), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_capture_copy_result_range(capture, id, 17, pixels, 0), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_capture_copy_result_range(capture, id, SIZE_MAX, pixels, 4),
			-EINVAL);
	KUNIT_EXPECT_EQ(test, drm_capture_copy_result_range(capture, id, 1, pixels, SIZE_MAX),
			-EINVAL);
	KUNIT_EXPECT_PTR_EQ(test, memchr_inv(pixels, 0xa9, sizeof(pixels)), NULL);
}

static void drm_capture_incomplete_ranges_preserve_output(struct kunit *test)
{
	struct drm_capture *capture = capture_create(test, 1);
	struct drm_capture_job *job;
	u8 pixels[4];
	u64 id;

	memset(pixels, 0xa9, sizeof(pixels));
	KUNIT_ASSERT_EQ(test, drm_capture_queue(capture, &id), 0);
	KUNIT_EXPECT_EQ(test, drm_capture_copy_result_range(capture, id, 0, pixels, 4), -EAGAIN);
	job = drm_capture_claim(capture);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, job);
	KUNIT_EXPECT_EQ(test, drm_capture_copy_result_range(capture, id, 0, pixels, 4), -EAGAIN);
	drm_capture_complete(job, -EIO);
	KUNIT_EXPECT_EQ(test, drm_capture_copy_result_range(capture, id, 0, pixels, 4), -EIO);
	KUNIT_EXPECT_EQ(test, drm_capture_copy_result_range(capture, id, 16, NULL, 0), -EIO);
	KUNIT_EXPECT_PTR_EQ(test, memchr_inv(pixels, 0xa9, sizeof(pixels)), NULL);
}

static void drm_capture_claim_selected_request(struct kunit *test)
{
	struct drm_capture *capture = capture_create(test, 2);
	struct drm_capture_job *job;
	u64 first, second;
	u8 pixels[16];

	KUNIT_ASSERT_EQ(test, drm_capture_queue(capture, &first), 0);
	KUNIT_ASSERT_EQ(test, drm_capture_queue(capture, &second), 0);
	job = drm_capture_claim_request(capture, second);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, job);
	memset(drm_capture_job_data(job), 0x29, sizeof(pixels));
	drm_capture_complete(job, 0);
	capture_expect_status(test, capture, first, false, -EINPROGRESS);
	capture_expect_status(test, capture, second, true, 0);
	KUNIT_EXPECT_EQ(test, drm_capture_copy_result(capture, second, pixels, sizeof(pixels)),
			sizeof(pixels));
	KUNIT_EXPECT_PTR_EQ(test, memchr_inv(pixels, 0x29, sizeof(pixels)), NULL);
	job = drm_capture_claim(capture);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, job);
	drm_capture_complete(job, -EIO);
	capture_expect_status(test, capture, first, true, -EIO);
}

static void drm_capture_selected_claim_does_not_substitute(struct kunit *test)
{
	struct drm_capture *capture = capture_create(test, 2);
	struct drm_capture_job *job;
	u64 first, second, replacement;

	KUNIT_ASSERT_EQ(test, drm_capture_queue(capture, &first), 0);
	KUNIT_ASSERT_EQ(test, drm_capture_queue(capture, &second), 0);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_capture_claim_request(capture, 0)), -ENOENT);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_capture_claim_request(capture, U64_MAX)), -ENOENT);
	KUNIT_ASSERT_EQ(test, drm_capture_cancel(capture, first), 0);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_capture_claim_request(capture, first)), -EALREADY);
	KUNIT_ASSERT_EQ(test, drm_capture_discard(capture, first), 0);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_capture_claim_request(capture, first)), -ENOENT);
	KUNIT_ASSERT_EQ(test, drm_capture_queue(capture, &replacement), 0);
	KUNIT_EXPECT_GT(test, replacement, second);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_capture_claim_request(capture, first)), -ENOENT);
	capture_expect_status(test, capture, second, false, -EINPROGRESS);
	capture_expect_status(test, capture, replacement, false, -EINPROGRESS);
	job = drm_capture_claim_request(capture, second);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, job);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_capture_claim_request(capture, second)), -EALREADY);
	drm_capture_complete(job, 0);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_capture_claim_request(capture, second)), -EALREADY);
}

static void drm_capture_selected_claim_retains_storage(struct kunit *test)
{
	struct drm_capture *capture = capture_create(test, 1);
	struct drm_capture_job *job;
	u64 id, next;

	KUNIT_ASSERT_EQ(test, drm_capture_queue(capture, &id), 0);
	job = drm_capture_claim_request(capture, id);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, job);
	KUNIT_EXPECT_EQ(test, drm_capture_discard(capture, id), 0);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_capture_claim_request(capture, id)), -ENOENT);
	KUNIT_EXPECT_EQ(test, drm_capture_queue(capture, &next), -EAGAIN);
	drm_capture_shutdown(capture);
	KUNIT_EXPECT_EQ(test, PTR_ERR(drm_capture_claim_request(capture, id)), -EKEYREVOKED);
	memset(drm_capture_job_data(job), 0x45, 16);
	drm_capture_complete(job, 0);
}

static struct kunit_case drm_capture_cases[] = {
	KUNIT_CASE(drm_capture_claim_selected_request),
	KUNIT_CASE(drm_capture_selected_claim_does_not_substitute),
	KUNIT_CASE(drm_capture_selected_claim_retains_storage),
	KUNIT_CASE(drm_capture_result_ranges),
	KUNIT_CASE(drm_capture_rejected_ranges_preserve_output),
	KUNIT_CASE(drm_capture_incomplete_ranges_preserve_output),
	KUNIT_CASE(drm_capture_credits),
	KUNIT_CASE(drm_capture_revoke_claimed),
	KUNIT_CASE(drm_capture_cancel_claimed),
	KUNIT_CASE(drm_capture_close_claimed),
	KUNIT_CASE(drm_capture_cancel_queued),
	KUNIT_CASE(drm_capture_independent_owners),
	KUNIT_CASE(drm_capture_put_preserves_other_owners),
	KUNIT_CASE(drm_capture_last_put_drains_requests),
	KUNIT_CASE(drm_capture_snapshot_isolation),
	KUNIT_CASE(drm_capture_failed_image),
	KUNIT_CASE(drm_capture_discard_unclaimed),
	KUNIT_CASE(drm_capture_discard_claimed),
	KUNIT_CASE(drm_capture_discard_survives_close),
	{}
};

static struct kunit_suite drm_capture_suite = {
	.name = "drm_capture",
	.test_cases = drm_capture_cases,
};

kunit_test_suite(drm_capture_suite);

MODULE_LICENSE("GPL");
