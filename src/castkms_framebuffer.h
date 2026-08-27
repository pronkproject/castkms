/* SPDX-License-Identifier: GPL-2.0-only */

#ifndef _CASTKMS_FRAMEBUFFER_H_
#define _CASTKMS_FRAMEBUFFER_H_

#include <linux/types.h>

struct drm_device;
struct drm_file;
struct drm_format_info;
struct drm_framebuffer;
struct drm_master;
struct drm_mode_fb_cmd2;

struct drm_framebuffer *
castkms_framebuffer_create(struct drm_device *dev, struct drm_file *file_priv,
			   const struct drm_format_info *info,
			   const struct drm_mode_fb_cmd2 *mode_cmd);

struct drm_master *
castkms_framebuffer_capture_owner(const struct drm_framebuffer *framebuffer,
				  const struct drm_master *current_master);

struct drm_master *
castkms_framebuffer_resolve_capture_owner(
	struct drm_master *capture_owner,
	struct drm_master *associated_owner,
	const struct drm_master *current_master);

bool castkms_framebuffer_capture_owners_match(
	const struct drm_master *owner,
	const struct drm_master *plane_owner);

#endif /* _CASTKMS_FRAMEBUFFER_H_ */
