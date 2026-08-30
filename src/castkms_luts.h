/* SPDX-License-Identifier: GPL-2.0+ */

#ifndef _CASTKMS_LUTS_H_
#define _CASTKMS_LUTS_H_

#include <linux/kconfig.h>

#include "castkms_frame.h"

#define LUT_SIZE 256

/* These values also index the first three fields of struct drm_color_lut. */
enum lut_channel {
	LUT_RED = 0,
	LUT_GREEN,
	LUT_BLUE,
	LUT_RESERVED,
};

extern const struct castkms_color_lut castkms_linear_eotf;
extern const struct castkms_color_lut castkms_srgb_eotf;
extern const struct castkms_color_lut castkms_srgb_inv_eotf;

u16 castkms_apply_lut_to_channel_value(const struct castkms_color_lut *lut,
				       s32 channel_value,
				       enum lut_channel channel);

#if IS_ENABLED(CONFIG_KUNIT)
u16 castkms_lerp_u16(u16 a, u16 b, s64 t);
s64 castkms_get_lut_index(const struct castkms_color_lut *lut,
			  u16 channel_value);
#endif
#endif /* _CASTKMS_LUTS_H_ */
