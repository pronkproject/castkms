/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_CONSTRAINTS_H__
#define __DRM_CONSTRAINTS_H__

#include <linux/types.h>

struct drm_constraints_description;

/* Kernel prototype limits, not an allocated userspace ABI. */
#define DRM_CONSTRAINTS_MAX_FORMATS 256
#define DRM_CONSTRAINTS_MAX_PROPERTIES 64

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

/**
 * struct drm_constraints_property - scalar rules using DRM property semantics
 * @object_id: existing CRTC or plane object ID
 * @property_id: property attached to that object
 * @type: DRM_MODE_PROP_RANGE, SIGNED_RANGE, ENUM or BITMASK
 * @minimum: inclusive range minimum; zero for enum/bitmask
 * @maximum: inclusive range maximum; zero for enum/bitmask
 * @mask: permitted enum values (bits 0..63) or permitted bitmask bits
 *
 * Signed ranges use the standard DRM two's-complement u64 representation.
 * Range rules require a zero mask. Enum rules require a nonempty mask and
 * describe values 0..63; bitmask rules may permit only zero. Blob contents,
 * object references and interactions between properties remain provider checks.
 * Rules apply to the enabled CRTC and planes used by the scene, not unused
 * objects. Their declared type must match the attached property's native type.
 */
struct drm_constraints_property {
	u32 object_id;
	u32 property_id;
	u32 type;
	u64 minimum;
	u64 maximum;
	u64 mask;
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
				   unsigned int count,
				   const struct drm_constraints_property *properties,
				   unsigned int property_count);
struct drm_constraints_description *
drm_constraints_description_get(struct drm_constraints_description *description);
void drm_constraints_description_put(struct drm_constraints_description *description);

/* Borrowed immutable views, valid until the caller drops its reference. */
const struct drm_constraints_size *
drm_constraints_description_output(const struct drm_constraints_description *description);
const struct drm_constraints_format *
drm_constraints_description_formats(const struct drm_constraints_description *description,
				    unsigned int *count);
const struct drm_constraints_property *
drm_constraints_description_properties(const struct drm_constraints_description *description,
				       unsigned int *count);

/* Test one scalar using the rule's declared native DRM property semantics. */
bool drm_constraints_property_matches(const struct drm_constraints_property *property, u64 value);

#endif
