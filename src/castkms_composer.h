/* SPDX-License-Identifier: GPL-2.0+ */

#ifndef _CASTKMS_COMPOSER_H_
#define _CASTKMS_COMPOSER_H_

#include <linux/kconfig.h>
#include <linux/types.h>

#include "castkms_frame.h"

struct castkms_output_buffer;

int castkms_compose_frame(const struct castkms_frame_stage *frame,
			  const struct castkms_output_buffer *destination);
int castkms_compose_targets(
	const struct castkms_frame_stage *frame,
	const struct castkms_output_buffer *destination,
	const struct castkms_output_buffer *second_destination, u32 *crc32);

#if IS_ENABLED(CONFIG_KUNIT)
void castkms_apply_colorops(const struct castkms_frame_plane *plane,
			    struct line_buffer *output_buffer);
void castkms_apply_3x4_matrix(struct pixel_argb_s32 *pixel, const struct drm_color_ctm_3x4 *matrix);
#endif

#endif /* _CASTKMS_COMPOSER_H_ */
