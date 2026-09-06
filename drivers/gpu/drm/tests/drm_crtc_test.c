// SPDX-License-Identifier: GPL-2.0

#include <drm/drm_atomic_state_helper.h>
#include <drm/drm_crtc.h>
#include <drm/drm_drv.h>
#include <drm/drm_kunit_helpers.h>
#include <drm/drm_plane.h>

#include <kunit/static_stub.h>
#include <kunit/test.h>

#include "../drm_crtc_internal.h"

static int fail_crc_init(struct drm_crtc *crtc)
{
	struct kunit *test = kunit_get_current_test();
	unsigned int *calls = test->priv;

	(*calls)++;
	return -ENOMEM;
}

static const struct drm_crtc_funcs crtc_funcs = {
	.destroy = drm_crtc_cleanup,
	.atomic_destroy_state = drm_atomic_helper_crtc_destroy_state,
	.atomic_duplicate_state = drm_atomic_helper_crtc_duplicate_state,
};

static void drm_crtc_crc_failure_unwinds(struct kunit *test)
{
	struct drm_device *drm;
	struct drm_plane *primary;
	struct drm_crtc *crtc;
	struct device *dev;
	unsigned int calls = 0;
	int ret;

	dev = drm_kunit_helper_alloc_device(test);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, dev);
	drm = __drm_kunit_helper_alloc_drm_device(test, dev, sizeof(*drm), 0,
						DRIVER_MODESET);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, drm);
	primary = drm_kunit_helper_create_primary_plane(test, drm, NULL, NULL,
						       NULL, 0, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, primary);
	crtc = kunit_kzalloc(test, sizeof(*crtc), GFP_KERNEL);
	KUNIT_ASSERT_NOT_NULL(test, crtc);

	test->priv = &calls;
	kunit_activate_static_stub(test, drm_crtc_crc_init, fail_crc_init);
	ret = drm_crtc_init_with_planes(drm, crtc, primary, NULL, &crtc_funcs,
					"crc-failure");
	kunit_deactivate_static_stub(test, drm_crtc_crc_init);
	test->priv = NULL;

	KUNIT_EXPECT_EQ(test, calls, 1);
	KUNIT_EXPECT_EQ(test, ret, -ENOMEM);
	KUNIT_EXPECT_TRUE(test, list_empty(&drm->mode_config.crtc_list));
	KUNIT_EXPECT_EQ(test, drm->mode_config.num_crtc, 0);
	KUNIT_EXPECT_EQ(test, primary->possible_crtcs, 0);

	/* Recover a published object if the regression returns, before releasing its storage. */
	if (!list_empty(&drm->mode_config.crtc_list))
		drm_crtc_cleanup(crtc);

	/* The same device must remain usable after the rejected initialization. */
	ret = drm_crtc_init_with_planes(drm, crtc, primary, NULL, &crtc_funcs,
					"crc-retry");
	KUNIT_EXPECT_EQ(test, ret, 0);
	if (!ret) {
		KUNIT_EXPECT_EQ(test, drm->mode_config.num_crtc, 1);
		KUNIT_EXPECT_EQ(test, crtc->index, 0);
		KUNIT_EXPECT_EQ(test, primary->possible_crtcs, 1);
		drm_crtc_cleanup(crtc);
	}
}

static struct kunit_case drm_crtc_tests[] = {
	KUNIT_CASE(drm_crtc_crc_failure_unwinds),
	{}
};

static struct kunit_suite drm_crtc_test_suite = {
	.name = "drm_crtc",
	.test_cases = drm_crtc_tests,
};

kunit_test_suite(drm_crtc_test_suite);

MODULE_DESCRIPTION("KUnit tests for DRM CRTC initialization");
MODULE_LICENSE("GPL");
MODULE_IMPORT_NS("EXPORTED_FOR_KUNIT_TESTING");
