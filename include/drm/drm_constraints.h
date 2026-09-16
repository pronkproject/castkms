/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_CONSTRAINTS_H__
#define __DRM_CONSTRAINTS_H__

#include <linux/types.h>

struct drm_constraints_description;

/* Kernel prototype limits, not an allocated userspace ABI. */
#define DRM_CONSTRAINTS_MAX_FORMATS 256

/**
 * struct drm_constraints_size - inclusive integer pixel dimensions
 * @min_width: smallest permitted width, nonzero
 * @min_height: smallest permitted height, nonzero
 * @max_width: largest permitted width
 * @max_height: largest permitted height
 *
 * Equal minimum and maximum dimensions describe an exact size. Source sizes
 * describe framebuffer allocation, not the fractional plane source rectangle.
 */
struct drm_constraints_size {
	u32 min_width;
	u32 min_height;
	u32 max_width;
	u32 max_height;
};

/**
 * struct drm_constraints_format - one plane's framebuffer allocation limits
 * @plane_id: existing DRM plane object ID
 * @format: DRM fourcc
 * @modifier: DRM format modifier, including linear
 * @size: permitted framebuffer dimensions for this format/modifier pair
 */
struct drm_constraints_format {
	u32 plane_id;
	u32 format;
	u64 modifier;
	struct drm_constraints_size size;
};

/*
 * Immutable, independently referenced allocation information. Creation copies
 * all input before returning; the caller retains ownership of its input.
 * Formats must contain distinct (plane_id, format, modifier) tuples. These
 * limits are necessary, not sufficient, for display: provider atomic checks
 * still validate complete scenes, standard properties and shared resources.
 * This object neither authorizes access nor retains the referenced KMS objects.
 * The registering provider must validate object membership and retain scope.
 * All reference operations require a live reference. Final put may sleep.
 */
struct drm_constraints_description *
drm_constraints_description_create(const struct drm_constraints_size *output,
				   const struct drm_constraints_format *formats,
				   unsigned int count);
struct drm_constraints_description *
drm_constraints_description_get(struct drm_constraints_description *description);
void drm_constraints_description_put(struct drm_constraints_description *description);

/* Borrowed immutable views, valid until the caller drops its reference. */
const struct drm_constraints_size *
drm_constraints_description_output(const struct drm_constraints_description *description);
const struct drm_constraints_format *
drm_constraints_description_formats(const struct drm_constraints_description *description,
				    unsigned int *count);

#endif
