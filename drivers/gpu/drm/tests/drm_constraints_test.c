// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/err.h>
#include <linux/module.h>
#include <drm/drm_constraints.h>
#include <drm/drm_fourcc.h>
#include <kunit/test.h>

static const struct drm_constraints_size output_size = { 1920, 1080, 1920, 1080 };
static const struct drm_constraints_format linear = {
	.plane_id = 17,
	.format = DRM_FORMAT_XRGB8888,
	.modifier = DRM_FORMAT_MOD_LINEAR,
	.size = { 1, 1, 8192, 8192 },
};

static void description_put(void *data)
{
	drm_constraints_description_put(data);
}

static struct drm_constraints_description *
create_description(struct kunit *test, const struct drm_constraints_size *output,
		   const struct drm_constraints_format *formats, unsigned int count)
{
	struct drm_constraints_description *description;

	description = drm_constraints_description_create(output, formats, count);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, description);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, description_put, description), 0);
	return description;
}

static void drm_constraints_copies_all_allocation_information(struct kunit *test)
{
	struct drm_constraints_size output = output_size;
	struct drm_constraints_format formats[] = { linear, linear };
	struct drm_constraints_description *description;
	const struct drm_constraints_format *view;
	unsigned int count;

	formats[1].modifier = I915_FORMAT_MOD_X_TILED;
	formats[1].size = output_size;
	description = create_description(test, &output, formats, ARRAY_SIZE(formats));
	memset(&output, 0, sizeof(output));
	memset(formats, 0, sizeof(formats));
	KUNIT_EXPECT_MEMEQ(test, drm_constraints_description_output(description),
			  &output_size, sizeof(output_size));
	view = drm_constraints_description_formats(description, &count);
	KUNIT_ASSERT_EQ(test, count, 2);
	KUNIT_EXPECT_MEMEQ(test, &view[0], &linear, sizeof(linear));
	KUNIT_EXPECT_EQ(test, view[1].modifier, I915_FORMAT_MOD_X_TILED);
	KUNIT_EXPECT_MEMEQ(test, &view[1].size, &output_size, sizeof(output_size));
}

static void drm_constraints_references_retain_description(struct kunit *test)
{
	struct drm_constraints_description *description;

	description = create_description(test, &output_size, &linear, 1);
	KUNIT_EXPECT_PTR_EQ(test, drm_constraints_description_get(description), description);
	drm_constraints_description_put(description);
	KUNIT_EXPECT_EQ(test, drm_constraints_description_output(description)->min_width, 1920);
}

static void drm_constraints_rejects_invalid_dimensions(struct kunit *test)
{
	const struct drm_constraints_size invalid[] = {
		{ 0, 1, 64, 64 }, { 1, 0, 64, 64 },
		{ 65, 1, 64, 64 }, { 1, 65, 64, 64 },
	};
	struct drm_constraints_format format = linear;
	unsigned int i;

	for (i = 0; i < ARRAY_SIZE(invalid); i++) {
		KUNIT_EXPECT_PTR_EQ(test,
			drm_constraints_description_create(&invalid[i], &linear, 1),
			ERR_PTR(-EINVAL));
		format.size = invalid[i];
		KUNIT_EXPECT_PTR_EQ(test,
			drm_constraints_description_create(&output_size, &format, 1),
			ERR_PTR(-EINVAL));
	}
}

static void drm_constraints_rejects_invalid_formats(struct kunit *test)
{
	struct drm_constraints_format formats[] = { linear, linear };

	KUNIT_EXPECT_PTR_EQ(test,
		drm_constraints_description_create(&output_size, formats, 2), ERR_PTR(-EEXIST));
	formats[0].plane_id = 0;
	KUNIT_EXPECT_PTR_EQ(test,
		drm_constraints_description_create(&output_size, formats, 1), ERR_PTR(-EINVAL));
	formats[0] = linear;
	formats[0].format = 0;
	KUNIT_EXPECT_PTR_EQ(test,
		drm_constraints_description_create(&output_size, formats, 1), ERR_PTR(-EINVAL));
	formats[0] = linear;
	formats[0].modifier = DRM_FORMAT_MOD_INVALID;
	KUNIT_EXPECT_PTR_EQ(test,
		drm_constraints_description_create(&output_size, formats, 1), ERR_PTR(-EINVAL));
	formats[0] = linear;
	formats[1].plane_id++;
	create_description(test, &output_size, formats, 2);
}

static void drm_constraints_bounds_input_before_access(struct kunit *test)
{
	KUNIT_EXPECT_PTR_EQ(test,
		drm_constraints_description_create(NULL, &linear, 1), ERR_PTR(-EINVAL));
	KUNIT_EXPECT_PTR_EQ(test,
		drm_constraints_description_create(&output_size, NULL, 1), ERR_PTR(-EINVAL));
	KUNIT_EXPECT_PTR_EQ(test,
		drm_constraints_description_create(&output_size, &linear, 0), ERR_PTR(-EINVAL));
	KUNIT_EXPECT_PTR_EQ(test,
		drm_constraints_description_create(&output_size, &linear,
						   DRM_CONSTRAINTS_MAX_FORMATS + 1),
		ERR_PTR(-EINVAL));
}

static struct kunit_case drm_constraints_tests[] = {
	KUNIT_CASE(drm_constraints_copies_all_allocation_information),
	KUNIT_CASE(drm_constraints_references_retain_description),
	KUNIT_CASE(drm_constraints_rejects_invalid_dimensions),
	KUNIT_CASE(drm_constraints_rejects_invalid_formats),
	KUNIT_CASE(drm_constraints_bounds_input_before_access),
	{}
};

static struct kunit_suite drm_constraints_test_suite = {
	.name = "drm_constraints",
	.test_cases = drm_constraints_tests,
};

kunit_test_suite(drm_constraints_test_suite);

MODULE_LICENSE("Dual MIT/GPL");
