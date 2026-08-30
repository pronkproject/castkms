/* SPDX-License-Identifier: GPL-2.0+ */

#ifndef _CASTKMS_DIRECT_COMPOSER_H_
#define _CASTKMS_DIRECT_COMPOSER_H_

#include <linux/types.h>

struct castkms_frame_stage;
struct castkms_output_buffer;

bool castkms_frame_can_direct_compose_xrgb8888(const struct castkms_frame_stage *frame,
	const struct castkms_output_buffer *destination,
	const struct castkms_output_buffer *second_destination);
void castkms_direct_compose_xrgb8888(const struct castkms_frame_stage *frame,
	const struct castkms_output_buffer *destination,
	const struct castkms_output_buffer *second_destination,
	bool gamma_lut_is_identity);

#endif /* _CASTKMS_DIRECT_COMPOSER_H_ */
