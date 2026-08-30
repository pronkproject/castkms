// SPDX-License-Identifier: GPL-2.0+

#include <linux/limits.h>
#include <linux/string.h>

#include <drm/drm_fourcc.h>
#include <drm/drm_framebuffer.h>
#include <drm/drm_rect.h>

#include <kunit/visibility.h>

#include "castkms_direct_composer.h"
#include "castkms_formats.h"
#include "castkms_frame.h"
#include "castkms_luts.h"
#include "castkms_output_buffer.h"

struct castkms_xrgb8888_gamma_map {
	u8 channel[LUT_RESERVED][U8_MAX + 1];
};

static bool castkms_plane_colorops_are_bypassed(const struct castkms_frame_plane *plane)
{
	for (size_t i = 0; i < plane->num_colorops; i++)
		if (!plane->colorops[i].bypass)
			return false;

	return true;
}

static bool castkms_output_is_xrgb8888(const struct castkms_output_buffer *output,
				       u32 width, u32 height)
{
	return !output ||
	       (output->fb->format->format == DRM_FORMAT_XRGB8888 &&
		output->fb->width == width && output->fb->height == height);
}

static bool
castkms_primary_plane_can_direct_compose_xrgb8888(
	const struct castkms_frame_stage *frame,
	const struct castkms_frame_plane *plane)
{
	const struct castkms_frame_info *info = plane->frame_info;
	const struct drm_rect *src, *dst;
	u32 format;

	if (!info || !info->fb || plane->is_cursor ||
	    !castkms_plane_colorops_are_bypassed(plane))
		return false;

	format = info->fb->format->format;
	if (format != DRM_FORMAT_XRGB8888 && format != DRM_FORMAT_XBGR8888)
		return false;

	src = &info->src;
	dst = &info->dst;
	if (info->rotation != DRM_MODE_ROTATE_0 ||
	    dst->x1 != 0 || dst->y1 != 0 || dst->x2 != frame->width ||
	    dst->y2 != frame->height)
		return false;

	if (src->x1 < 0 || src->y1 < 0 ||
	    (src->x1 & 0xffff) || (src->y1 & 0xffff) ||
	    (s64)src->x2 - src->x1 != (s64)frame->width << 16 ||
	    (s64)src->y2 - src->y1 != (s64)frame->height << 16 ||
	    src->x2 > (s64)info->fb->width << 16 ||
	    src->y2 > (s64)info->fb->height << 16)
		return false;

	return true;
}

static bool
castkms_cursor_plane_can_direct_compose_xrgb8888(
	const struct castkms_frame_stage *frame,
	const struct castkms_frame_plane *plane)
{
	const struct castkms_frame_info *info = plane->frame_info;
	const struct drm_rect *src, *dst;

	if (!info || !info->fb || !plane->is_cursor ||
	    !castkms_plane_colorops_are_bypassed(plane) ||
	    info->fb->format->format != DRM_FORMAT_ARGB8888)
		return false;

	src = &info->src;
	dst = &info->dst;
	if (info->rotation != DRM_MODE_ROTATE_0 ||
	    dst->x2 <= dst->x1 || dst->y2 <= dst->y1)
		return false;

	if (src->x1 < 0 || src->y1 < 0 ||
	    (src->x1 & 0xffff) || (src->y1 & 0xffff) ||
	    (s64)src->x2 - src->x1 != ((s64)dst->x2 - dst->x1) << 16 ||
	    (s64)src->y2 - src->y1 != ((s64)dst->y2 - dst->y1) << 16 ||
	    src->x2 > (s64)info->fb->width << 16 ||
	    src->y2 > (s64)info->fb->height << 16)
		return false;

	/* Gamma is defined after blending, so the packed cursor path starts with
	 * the common identity case rather than changing color-pipeline semantics.
	 */
	return castkms_color_lut_is_identity(&frame->gamma_lut);
}

VISIBLE_IF_KUNIT bool
castkms_frame_can_direct_compose_xrgb8888(const struct castkms_frame_stage *frame,
	const struct castkms_output_buffer *destination,
	const struct castkms_output_buffer *second_destination)
{
	if ((!destination && !second_destination) ||
	    (frame->num_planes != 1 && frame->num_planes != 2))
		return false;

	if (!castkms_output_is_xrgb8888(destination,
					frame->width, frame->height) ||
	    !castkms_output_is_xrgb8888(second_destination,
					 frame->width, frame->height))
		return false;

	if (!castkms_primary_plane_can_direct_compose_xrgb8888(
		    frame, frame->planes[0]))
		return false;

	if (frame->num_planes == 2)
		return castkms_cursor_plane_can_direct_compose_xrgb8888(
			frame, frame->planes[1]);

	return true;
}
EXPORT_SYMBOL_IF_KUNIT(castkms_frame_can_direct_compose_xrgb8888);

static u8 *castkms_packed_pixel_address(const struct drm_framebuffer *fb,
					const struct iosys_map *map,
					int x, int y)
{
	int rem_x, rem_y;
	size_t offset;

	offset = castkms_packed_pixels_offset(fb, x, y, 0, &rem_x, &rem_y);
	return (u8 *)map[0].vaddr + offset;
}

static void
castkms_xrgb8888_gamma_map_init(struct castkms_xrgb8888_gamma_map *map,
	const struct castkms_color_lut *lut)
{
	for (unsigned int value = 0; value <= U8_MAX; value++) {
		u16 input = value * 0x101;

		for (enum lut_channel channel = LUT_RED;
		     channel < LUT_RESERVED; channel++) {
			u16 output = castkms_apply_lut_to_channel_value(lut,
								  input, channel);

			map->channel[channel][value] =
				DIV_ROUND_CLOSEST(output, 0x101);
		}
	}
}

static void castkms_copy_xrgb8888_row(u8 *dst, const u8 *src,
	size_t pixel_count, bool source_is_xbgr,
	const struct castkms_xrgb8888_gamma_map *gamma)
{
	if (!source_is_xbgr && !gamma) {
		if (dst != src)
			memcpy(dst, src, pixel_count * 4);
		return;
	}

	for (size_t x = 0; x < pixel_count; x++) {
		u8 red = source_is_xbgr ? src[0] : src[2];
		u8 green = src[1];
		u8 blue = source_is_xbgr ? src[2] : src[0];

		if (gamma) {
			red = gamma->channel[LUT_RED][red];
			green = gamma->channel[LUT_GREEN][green];
			blue = gamma->channel[LUT_BLUE][blue];
		}

		dst[0] = blue;
		dst[1] = green;
		dst[2] = red;
		dst[3] = 0xff;
		src += 4;
		dst += 4;
	}
}

static u8 castkms_blend_premultiplied_u8(u8 src, u8 dst, u8 alpha)
{
	u16 src_u16 = (u16)src * 0x101;
	u16 dst_u16 = (u16)dst * 0x101;
	u16 alpha_u16 = (u16)alpha * 0x101;
	u32 blended;

	/* Match the reference composer's 16-bit blend and final 8-bit packing. */
	blended = src_u16 * 0xffffU + dst_u16 * (0xffffU - alpha_u16);
	blended = DIV_ROUND_CLOSEST(blended, 0xffffU);
	return DIV_ROUND_CLOSEST(blended, 0x101U);
}

static void castkms_blend_argb8888_cursor_row(u8 *dst, const u8 *src,
					      size_t pixel_count)
{
	for (size_t x = 0; x < pixel_count; x++) {
		u8 alpha = src[3];

		if (alpha == U8_MAX) {
			dst[0] = src[0];
			dst[1] = src[1];
			dst[2] = src[2];
		} else if (alpha) {
			dst[0] = castkms_blend_premultiplied_u8(src[0], dst[0], alpha);
			dst[1] = castkms_blend_premultiplied_u8(src[1], dst[1], alpha);
			dst[2] = castkms_blend_premultiplied_u8(src[2], dst[2], alpha);
		}
		dst[3] = 0xff;
		src += 4;
		dst += 4;
	}
}

static void castkms_direct_compose_argb8888_cursor(
	const struct castkms_frame_stage *frame,
	const struct castkms_output_buffer *primary,
	const struct castkms_output_buffer *secondary)
{
	const struct castkms_frame_info *info = frame->planes[1]->frame_info;
	const struct drm_rect *dst = &info->dst;
	int dst_x1 = max(dst->x1, 0);
	int dst_y1 = max(dst->y1, 0);
	int dst_x2 = min_t(int, dst->x2, frame->width);
	int dst_y2 = min_t(int, dst->y2, frame->height);
	int src_x, src_y;
	size_t row_size;

	if (dst_x1 >= dst_x2 || dst_y1 >= dst_y2)
		return;

	src_x = (info->src.x1 >> 16) + dst_x1 - dst->x1;
	src_y = (info->src.y1 >> 16) + dst_y1 - dst->y1;
	row_size = (size_t)(dst_x2 - dst_x1) * 4;

	for (int y = dst_y1; y < dst_y2; y++) {
		const u8 *src = castkms_packed_pixel_address(info->fb, info->map,
							   src_x,
							   src_y + y - dst_y1);
		u8 *dst_row = castkms_packed_pixel_address(primary->fb,
							primary->map, dst_x1, y);

		castkms_blend_argb8888_cursor_row(dst_row, src,
						      dst_x2 - dst_x1);
		if (secondary) {
			u8 *second_dst = castkms_packed_pixel_address(
				secondary->fb, secondary->map, dst_x1, y);

			if (second_dst != dst_row)
				memcpy(second_dst, dst_row, row_size);
		}
	}
}

void castkms_direct_compose_xrgb8888(const struct castkms_frame_stage *frame,
	const struct castkms_output_buffer *destination,
	const struct castkms_output_buffer *second_destination,
	bool gamma_lut_is_identity)
{
	const struct castkms_frame_info *info = frame->planes[0]->frame_info;
	const struct castkms_output_buffer *primary = destination ?
		destination : second_destination;
	const struct castkms_output_buffer *secondary =
		destination && second_destination ? second_destination : NULL;
	struct castkms_xrgb8888_gamma_map gamma_map;
	const struct castkms_xrgb8888_gamma_map *gamma = NULL;
	bool source_is_xbgr =
		info->fb->format->format == DRM_FORMAT_XBGR8888;
	int src_x = info->src.x1 >> 16;
	int src_y = info->src.y1 >> 16;
	size_t row_size = (size_t)frame->width * 4;

	if (!gamma_lut_is_identity) {
		castkms_xrgb8888_gamma_map_init(&gamma_map, &frame->gamma_lut);
		gamma = &gamma_map;
	}

	for (u32 y = 0; y < frame->height; y++) {
		const u8 *src = castkms_packed_pixel_address(info->fb,
			info->map, src_x, src_y + y);
		u8 *dst = castkms_packed_pixel_address(primary->fb,
			primary->map, 0, y);

		castkms_copy_xrgb8888_row(dst, src, frame->width,
					  source_is_xbgr, gamma);
		if (secondary) {
			u8 *second_dst = castkms_packed_pixel_address(secondary->fb,
				secondary->map, 0, y);

			if (second_dst != dst)
				memcpy(second_dst, dst, row_size);
		}
	}

	if (frame->num_planes == 2)
		castkms_direct_compose_argb8888_cursor(frame, primary, secondary);
}
