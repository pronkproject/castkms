// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <drm/drm_atomic.h>
#include <drm/drm_kunit_helpers.h>
#include <drm/drm_modeset_lock.h>
#include <drm/drm_property.h>
#include <kunit/test.h>

#include "../drm_atomic_user_input.h"
#include "../drm_atomic_user_request.h"

struct fence_fixture {
	struct drm_device *dev;
	struct drm_crtc *crtc;
	struct drm_atomic_user_request *request;
	unsigned long applied_address;
	bool included_crtc;
};

static void free_request(void *data)
{
	drm_atomic_free_user_request(data);
}

static int lock_display(struct drm_device *dev, struct drm_modeset_acquire_ctx *ctx)
{
	int ret;

	for (;;) {
		ret = drm_modeset_lock_all_ctx(dev, ctx);
		if (ret != -EDEADLK)
			return ret;
		ret = drm_modeset_backoff(ctx);
		if (ret)
			return ret;
	}
}

static struct fence_fixture *new_fixture(struct kunit *test, const u64 addresses[3])
{
	struct fence_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct device *parent = drm_kunit_helper_alloc_device(test);
	struct drm_modeset_acquire_ctx ctx;
	struct drm_atomic_user_input input;
	struct drm_plane *plane;
	u32 object, count = 3, properties[3];
	int ret;

	KUNIT_ASSERT_NOT_NULL(test, f);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	f->dev = __drm_kunit_helper_alloc_drm_device(test, parent, sizeof(*f->dev), 0,
						  DRIVER_MODESET | DRIVER_ATOMIC);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->dev);
	plane = drm_kunit_helper_create_primary_plane(test, f->dev, NULL, NULL, NULL, 0, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, plane);
	f->crtc = drm_kunit_helper_create_crtc(test, f->dev, plane, NULL, NULL, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->crtc);
	drm_mode_config_reset(f->dev);
	object = f->crtc->base.id;
	properties[0] = properties[1] = properties[2] = f->dev->mode_config.prop_out_fence_ptr->base.id;
	input = (struct drm_atomic_user_input) {
		.object_count = 1, .property_count = 3, .objects = &object, .counts = &count,
		.properties = properties, .values = addresses,
	};
	drm_modeset_acquire_init(&ctx, 0);
	ret = lock_display(f->dev, &ctx);
	f->request = ret ? ERR_PTR(ret) : drm_atomic_resolve_user_request(f->dev, NULL, &input);
	drm_modeset_drop_locks(&ctx);
	drm_modeset_acquire_fini(&ctx);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->request);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, free_request, f->request), 0);
	return f;
}

static int apply_destinations(struct fence_fixture *f)
{
	struct drm_atomic_commit *state = drm_atomic_commit_alloc(f->dev);
	struct drm_modeset_acquire_ctx ctx;
	int ret;

	if (!state)
		return -ENOMEM;
	drm_modeset_acquire_init(&ctx, 0);
	state->acquire_ctx = &ctx;
	ret = lock_display(f->dev, &ctx);
	if (!ret)
		ret = drm_atomic_apply_user_fence_destinations(f->request, state);
	f->included_crtc = !!drm_atomic_get_new_crtc_state(state, f->crtc);
	f->applied_address = (unsigned long)state->crtcs[drm_crtc_index(f->crtc)].out_fence_ptr;
	drm_atomic_commit_put(state);
	drm_modeset_drop_locks(&ctx);
	drm_modeset_acquire_fini(&ctx);
	return ret;
}

static void last_nonnull_destination_is_reapplied(struct kunit *test)
{
	u64 addresses[] = { 0x1000, 0x2000, 0 };
	struct fence_fixture *f = new_fixture(test, addresses);
	unsigned int i;

	for (i = 0; i < 2; i++) {
		KUNIT_ASSERT_EQ(test, apply_destinations(f), 0);
		KUNIT_EXPECT_TRUE(test, f->included_crtc);
		KUNIT_EXPECT_EQ(test, f->applied_address, 0x2000);
	}
}

static struct kunit_case cases[] = {
	KUNIT_CASE(last_nonnull_destination_is_reapplied),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_user_fence",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_IMPORT_NS("EXPORTED_FOR_KUNIT_TESTING");
MODULE_LICENSE("GPL");
