/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_CONSTRAINTS_H__
#define __DRM_CONSTRAINTS_H__

#include <linux/types.h>

struct drm_constraints_description;

/* Kernel prototype limits, not an allocated userspace ABI. */
#define DRM_CONSTRAINTS_MAX_FORMATS 4096
#define DRM_CONSTRAINTS_MAX_PROPERTIES 64
#define DRM_CONSTRAINTS_MAX_PLANE_LIMITS 64
#define DRM_CONSTRAINTS_MAX_PLANES_PER_LIMIT 64
#define DRM_CONSTRAINTS_FORMAT_IMPLICIT (1U << 0)
#define DRM_CONSTRAINTS_FORMAT_STORAGE_NATIVE 1U
#define DRM_CONSTRAINTS_FORMAT_STORAGE_IMPORTED 2U

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
 * @modifier: explicit DRM format modifier, or zero for implicit layout
 * @size: permitted framebuffer dimensions for this format/modifier pair
 * @flags: DRM_CONSTRAINTS_FORMAT_IMPLICIT for layout without FB_MODIFIERS, or zero
 * @storage_flags: permitted DRM_CONSTRAINTS_FORMAT_STORAGE_* origins
 * @pitch_alignment: required byte alignment of every plane pitch
 * @offset_alignment: required byte alignment of every plane offset
 * @max_pitch: maximum pitch in bytes for every plane
 *
 * Implicit layout does not promise linear storage. A zero modifier with zero
 * flags describes explicit LINEAR; the same modifier with IMPLICIT describes
 * framebuffer creation without DRM_MODE_FB_MODIFIERS. Storage flags apply to
 * every memory plane. Alignments are nonzero powers of two. Other flags are
 * invalid.
 */
struct drm_constraints_format {
	u32 plane_id;
	u32 format;
	u64 modifier;
	struct drm_constraints_size size;
	u32 flags;
	u32 storage_flags;
	u32 pitch_alignment;
	u32 offset_alignment;
	u32 max_pitch;
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
 * Rules describe standard scalar scene properties, not request-only sentinels
 * or driver-private properties. They apply to enabled CRTCs and used planes,
 * not unused objects. Their type must match the attached property's native type.
 */
struct drm_constraints_property {
	u32 object_id;
	u32 property_id;
	u32 type;
	u64 minimum;
	u64 maximum;
	u64 mask;
};

/**
 * struct drm_constraints_plane_limit - one overlapping active-plane ceiling
 * @max_active: maximum simultaneously used planes from this group
 * @count: number of distinct plane IDs
 * @plane_ids: borrowed input IDs, copied by description creation
 *
 * Every group applies. A plane can therefore participate in an overall limit
 * and in narrower resource or role limits. Counts and IDs are bounded by the
 * DRM_CONSTRAINTS_MAX_* constants.
 */
struct drm_constraints_plane_limit {
	u32 max_active;
	u32 count;
	const u32 *plane_ids;
};

/*
 * Immutable, independently referenced allocation, scalar-property and
 * active-plane-limit information. Creation copies all input before returning;
 * the caller retains ownership of its input.
 * Formats must contain distinct (plane_id, format, modifier, flags) tuples. These
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
				   unsigned int property_count,
				   const struct drm_constraints_plane_limit *plane_limits,
				   unsigned int plane_limit_count);
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
const struct drm_constraints_plane_limit *
drm_constraints_description_plane_limits(const struct drm_constraints_description *description,
					 unsigned int *count);

/* Test one scalar using the rule's declared native DRM property semantics. */
bool drm_constraints_property_matches(const struct drm_constraints_property *property, u64 value);

#endif
