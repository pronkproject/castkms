/* SPDX-License-Identifier: GPL-2.0+ */

#ifndef _CASTKMS_DISPLAY_IDENTITY_H_
#define _CASTKMS_DISPLAY_IDENTITY_H_

#include <linux/stddef.h>
#include <linux/types.h>

struct drm_edid;

bool castkms_display_identity_product_name(const struct drm_edid *drm_edid,
					   char *name, size_t name_size);
bool castkms_display_identity_product_name_from_raw(const u8 *edid,
						     size_t edid_size,
						     char *name,
						     size_t name_size);

#endif /* _CASTKMS_DISPLAY_IDENTITY_H_ */
