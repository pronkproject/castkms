// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <drm/drm_atomic_request.h>
#include <drm/drm_crtc.h>
#include <drm/drm_kunit_helpers.h>
#include <drm/drm_modeset_lock.h>
#include <drm/drm_property.h>
#include <kunit/test.h>

#include "../drm_atomic_user_input.h"
#include "../drm_atomic_user_request.h"

struct user_request_fixture {
	struct drm_device *dev;
	struct drm_plane *plane;
	struct drm_crtc *crtc;
};

static struct user_request_fixture *new_fixture(struct kunit *test)
{
	struct user_request_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct device *parent = drm_kunit_helper_alloc_device(test);

	KUNIT_ASSERT_NOT_NULL(test, f);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	f->dev = __drm_kunit_helper_alloc_drm_device(test, parent, sizeof(*f->dev), 0,
						  DRIVER_MODESET | DRIVER_ATOMIC);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->dev);
	f->plane = drm_kunit_helper_create_primary_plane(test, f->dev, NULL, NULL, NULL, 0, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->plane);
	f->crtc = drm_kunit_helper_create_crtc(test, f->dev, f->plane, NULL, NULL, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->crtc);
	return f;
}

static void free_request(void *data)
{
	drm_atomic_free_user_request(data);
}

static struct drm_atomic_user_request *resolve_request(struct drm_device *dev,
						       const struct drm_atomic_user_input *input)
{
	struct drm_modeset_acquire_ctx ctx;
	struct drm_atomic_user_request *request;
	int ret;

	drm_modeset_acquire_init(&ctx, 0);
	for (;;) {
		ret = drm_modeset_lock_all_ctx(dev, &ctx);
		if (ret != -EDEADLK)
			break;
		ret = drm_modeset_backoff(&ctx);
		if (ret)
			break;
	}
	request = ret ? ERR_PTR(ret) : drm_atomic_resolve_user_request(dev, NULL, input);
	drm_modeset_drop_locks(&ctx);
	drm_modeset_acquire_fini(&ctx);
	return request;
}

static struct drm_atomic_user_request *new_request(struct kunit *test, struct drm_device *dev,
						   const struct drm_atomic_user_input *input)
{
	struct drm_atomic_user_request *request = resolve_request(dev, input);

	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, request);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, free_request, request), 0);
	return request;
}

static void empty_request_has_no_targets_or_values(struct kunit *test)
{
	struct user_request_fixture *f = new_fixture(test);
	struct drm_atomic_user_input input = {};
	struct drm_atomic_user_request *request = new_request(test, f->dev, &input);

	KUNIT_EXPECT_EQ(test, drm_atomic_user_request_target_count(request), 0);
	KUNIT_EXPECT_PTR_EQ(test, drm_atomic_user_request_target(request, 0), NULL);
	KUNIT_EXPECT_EQ(test, drm_atomic_request_count(drm_atomic_user_request_values(request)), 0);
}

static struct kunit_case cases[] = {
	KUNIT_CASE(empty_request_has_no_targets_or_values),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_user_request",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_IMPORT_NS("EXPORTED_FOR_KUNIT_TESTING");
MODULE_LICENSE("GPL");
