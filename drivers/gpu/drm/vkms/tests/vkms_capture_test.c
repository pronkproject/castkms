// SPDX-License-Identifier: GPL-2.0+

#include <kunit/test.h>
#include <drm/drm_capture.h>
#include <drm/drm_capture_authority.h>
#include <drm/drm_fixed.h>
#include <drm/drm_framebuffer.h>

#include "../vkms_composer.h"

struct capture_context {
	struct drm_capture_authority *authority;
	struct drm_capture *stream;
	struct vkms_crtc_state state;
	struct vkms_plane_state plane;
	struct vkms_plane_state *planes[1];
	struct vkms_frame_info frame;
	struct drm_framebuffer fb;
	struct pixel_argb_u16 pixels[4];
	unsigned int reads;
	int policy_status;
	bool revoke_on_read;
};

static void capture_revoke(void *data)
{
	/* The authority registry owns revocation of the test stream. */
}

static int capture_authorize(void *data, struct drm_capture *stream)
{
	struct capture_context *context = data;

	return context->policy_status;
}

static const struct drm_capture_authority_ops capture_ops = {
	.owner = THIS_MODULE,
	.revoke = capture_revoke,
	.authorize_capture = capture_authorize,
};

static void capture_put(void *stream)
{
	drm_capture_put(stream);
}

static void authority_put(void *authority)
{
	drm_capture_authority_put(authority);
}

static int capture_test_init(struct kunit *test)
{
	struct capture_context *context;
	int ret;

	context = kunit_kzalloc(test, sizeof(*context), GFP_KERNEL);
	KUNIT_ASSERT_NOT_NULL(test, context);
	test->priv = context;
	context->state.base.mode.hdisplay = 2;
	context->state.base.mode.vdisplay = 2;
	context->state.base.background_color = 0xffff123456789abcULL;

	context->stream = drm_capture_create(2, sizeof(context->pixels));
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, context->stream);
	ret = kunit_add_action_or_reset(test, capture_put, context->stream);
	KUNIT_ASSERT_EQ(test, ret, 0);
	context->authority = drm_capture_authority_create(&capture_ops, context);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, context->authority);
	ret = kunit_add_action_or_reset(test, authority_put, context->authority);
	KUNIT_ASSERT_EQ(test, ret, 0);
	ret = drm_capture_authority_begin(context->authority);
	KUNIT_ASSERT_EQ(test, ret, 0);
	ret = drm_capture_authority_add_stream_locked(context->authority, context->stream);
	drm_capture_authority_end(context->authority);
	KUNIT_ASSERT_EQ(test, ret, 0);
	return 0;
}

static void read_source(const struct vkms_plane_state *plane, int x, int y,
			enum pixel_read_direction direction, int count,
			struct pixel_argb_u16 out[])
{
	struct capture_context *context = container_of(plane, struct capture_context, plane);

	context->reads++;
	memcpy(out, context->pixels + y * 2 + x, count * sizeof(*out));
	if (context->revoke_on_read)
		drm_capture_authority_revoke(context->authority);
}

static void prepare_source(struct capture_context *context)
{
	context->fb.width = 2;
	context->fb.height = 2;
	context->frame.fb = &context->fb;
	context->frame.src = DRM_RECT_INIT(0, 0, 2 << 16, 2 << 16);
	context->frame.dst = DRM_RECT_INIT(0, 0, 2, 2);
	context->frame.rotation = DRM_MODE_ROTATE_0;
	iosys_map_set_vaddr(&context->frame.map[0], context->pixels);
	context->plane.frame_info = &context->frame;
	context->plane.pixel_read_line = read_source;
	context->planes[0] = &context->plane;
	context->state.active_planes = context->planes;
	context->state.num_active_planes = 1;
}

static void capture_background_test(struct kunit *test)
{
	struct capture_context *context = test->priv;
	struct pixel_argb_u16 result[4];
	u64 id;

	KUNIT_ASSERT_EQ(test, drm_capture_queue(context->stream, &id), 0);
	KUNIT_ASSERT_EQ(test, vkms_composer_capture(&context->state, context->authority,
						    context->stream), 0);
	context->state.base.background_color = 0;
	KUNIT_ASSERT_EQ(test, drm_capture_copy_result(context->stream, id, result,
						      sizeof(result)), sizeof(result));
	for (int i = 0; i < ARRAY_SIZE(result); i++) {
		KUNIT_EXPECT_EQ(test, result[i].a, 0xffff);
		KUNIT_EXPECT_EQ(test, result[i].r, 0x1234);
		KUNIT_EXPECT_EQ(test, result[i].g, 0x5678);
		KUNIT_EXPECT_EQ(test, result[i].b, 0x9abc);
	}
	KUNIT_EXPECT_EQ(test, drm_capture_ack(context->stream, id), 0);
}

static void capture_source_test(struct kunit *test)
{
	struct capture_context *context = test->priv;
	struct pixel_argb_u16 result[4], expected[4];
	u64 id;

	prepare_source(context);
	for (int i = 0; i < ARRAY_SIZE(expected); i++)
		expected[i] = (struct pixel_argb_u16){ 0xffff, i * 100, i * 200, i * 300 };
	memcpy(context->pixels, expected, sizeof(expected));
	KUNIT_ASSERT_EQ(test, drm_capture_queue(context->stream, &id), 0);
	KUNIT_ASSERT_EQ(test, vkms_composer_capture(&context->state, context->authority,
						    context->stream), 0);
	KUNIT_EXPECT_EQ(test, context->reads, 2);
	memset(context->pixels, 0, sizeof(context->pixels));
	KUNIT_ASSERT_EQ(test, drm_capture_copy_result(context->stream, id, result,
						      sizeof(result)), sizeof(result));
	KUNIT_EXPECT_MEMEQ(test, result, expected, sizeof(result));
}

static void capture_gamma_test(struct kunit *test)
{
	struct capture_context *context = test->priv;
	struct drm_color_lut lut[] = { { 0xffff, 0xffff, 0xffff }, { 0, 0, 0 } };
	struct pixel_argb_u16 result[4];
	u64 id;

	context->state.base.background_color = 0xffff0000ffff0000ULL;
	context->state.gamma_lut.base = lut;
	context->state.gamma_lut.lut_length = ARRAY_SIZE(lut);
	context->state.gamma_lut.channel_value2index_ratio =
		drm_fixp_div(drm_int2fixp(1), drm_int2fixp(0xffff));
	KUNIT_ASSERT_EQ(test, drm_capture_queue(context->stream, &id), 0);
	KUNIT_ASSERT_EQ(test, vkms_composer_capture(&context->state, context->authority,
						    context->stream), 0);
	KUNIT_ASSERT_EQ(test, drm_capture_copy_result(context->stream, id, result,
						      sizeof(result)), sizeof(result));
	for (int i = 0; i < ARRAY_SIZE(result); i++) {
		KUNIT_EXPECT_EQ(test, result[i].a, 0xffff);
		KUNIT_EXPECT_EQ(test, result[i].r, 0xffff);
		/* Fixed-point LUT indexing may leave one unit at the far endpoint. */
		KUNIT_EXPECT_LE(test, result[i].g, 1);
		KUNIT_EXPECT_EQ(test, result[i].b, 0xffff);
	}
}

static void capture_admission_test(struct kunit *test)
{
	struct capture_context *context = test->priv;
	struct drm_capture_result result;
	u64 id;

	prepare_source(context);
	KUNIT_EXPECT_EQ(test, vkms_composer_capture(&context->state, context->authority,
						    context->stream), -EAGAIN);
	KUNIT_ASSERT_EQ(test, drm_capture_queue(context->stream, &id), 0);
	context->policy_status = -EACCES;
	KUNIT_EXPECT_EQ(test, vkms_composer_capture(&context->state, context->authority,
						    context->stream), -EACCES);
	KUNIT_EXPECT_EQ(test, context->reads, 0);
	KUNIT_ASSERT_EQ(test, drm_capture_query(context->stream, id, &result), 0);
	KUNIT_EXPECT_FALSE(test, result.completed);
	context->policy_status = 0;
	KUNIT_EXPECT_EQ(test, vkms_composer_capture(&context->state, context->authority,
						    context->stream), 0);
	KUNIT_EXPECT_EQ(test, context->reads, 2);
}

static void capture_size_test(struct kunit *test)
{
	struct capture_context *context = test->priv;
	struct drm_capture_result result;
	const u16 sizes[][2] = { { 0, 2 }, { 2, 0 }, { 1, 2 }, { 3, 2 }, { U16_MAX, U16_MAX } };
	u64 id;

	prepare_source(context);
	for (int i = 0; i < ARRAY_SIZE(sizes); i++) {
		context->state.base.mode.hdisplay = sizes[i][0];
		context->state.base.mode.vdisplay = sizes[i][1];
		KUNIT_ASSERT_EQ(test, drm_capture_queue(context->stream, &id), 0);
		KUNIT_EXPECT_EQ(test, vkms_composer_capture(&context->state, context->authority,
							    context->stream), -EINVAL);
		KUNIT_ASSERT_EQ(test, drm_capture_query(context->stream, id, &result), 0);
		KUNIT_EXPECT_TRUE(test, result.completed);
		KUNIT_EXPECT_EQ(test, result.status, -EINVAL);
		KUNIT_EXPECT_EQ(test, drm_capture_ack(context->stream, id), 0);
	}
	KUNIT_EXPECT_EQ(test, context->reads, 0);
}

static void capture_revoke_test(struct kunit *test)
{
	struct capture_context *context = test->priv;
	struct drm_capture_result result;
	struct pixel_argb_u16 output[4] = {}, expected[4] = {};
	u64 id;

	prepare_source(context);
	context->revoke_on_read = true;
	KUNIT_ASSERT_EQ(test, drm_capture_queue(context->stream, &id), 0);
	KUNIT_ASSERT_EQ(test, vkms_composer_capture(&context->state, context->authority,
						    context->stream), 0);
	KUNIT_ASSERT_EQ(test, drm_capture_query(context->stream, id, &result), 0);
	KUNIT_EXPECT_TRUE(test, result.completed);
	KUNIT_EXPECT_EQ(test, result.status, -EKEYREVOKED);
	KUNIT_EXPECT_EQ(test, drm_capture_copy_result(context->stream, id, output,
						      sizeof(output)), -EKEYREVOKED);
	KUNIT_EXPECT_MEMEQ(test, output, expected, sizeof(output));
	KUNIT_EXPECT_EQ(test, vkms_composer_capture(&context->state, context->authority,
						    context->stream), -EKEYREVOKED);
	KUNIT_EXPECT_EQ(test, context->reads, 2);
}

static struct kunit_case capture_cases[] = {
	KUNIT_CASE(capture_background_test),
	KUNIT_CASE(capture_source_test),
	KUNIT_CASE(capture_gamma_test),
	KUNIT_CASE(capture_admission_test),
	KUNIT_CASE(capture_size_test),
	KUNIT_CASE(capture_revoke_test),
	{}
};

static struct kunit_suite capture_suite = {
	.name = "vkms-capture",
	.init = capture_test_init,
	.test_cases = capture_cases,
};

kunit_test_suite(capture_suite);

MODULE_IMPORT_NS("EXPORTED_FOR_KUNIT_TESTING");
MODULE_LICENSE("GPL");
MODULE_DESCRIPTION("VKMS kernel capture tests");
