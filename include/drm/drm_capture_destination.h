/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_CAPTURE_DESTINATION_H__
#define __DRM_CAPTURE_DESTINATION_H__

#include <linux/types.h>

struct dma_buf;

#define DRM_CAPTURE_DESTINATION_MAX_PLANES 4

/**
 * struct drm_capture_destination_plane - borrowed storage for one image plane
 * @buffer: live DMA-BUF retained by the caller throughout validation and use
 * @offset: byte offset within that allocation
 * @stride: byte stride interpreted according to the format and modifier
 *
 * Several planes may name the same allocation. These are image planes, not
 * KMS plane objects. A descriptor number is never used as storage identity.
 */
struct drm_capture_destination_plane {
	struct dma_buf *buffer;
	u64 offset;
	u32 stride;
};

/**
 * struct drm_capture_destination - borrowed final-image storage description
 * @width: nonzero visible width in pixels
 * @height: nonzero visible height in pixels
 * @format: nonzero DRM fourcc
 * @num_planes: number of entries in planes, from one through four
 * @modifier: explicit DRM format modifier, not DRM_FORMAT_MOD_INVALID
 * @planes: image storage; entries beyond num_planes are ignored
 *
 * This description owns no references and grants no capture permission. The
 * caller keeps every buffer alive and all metadata stable throughout use.
 * Providers retaining storage must acquire their own buffer references.
 * Registration is neither a map nor a reservation against competing accesses.
 */
struct drm_capture_destination {
	u32 width;
	u32 height;
	u32 format;
	u32 num_planes;
	u64 modifier;
	struct drm_capture_destination_plane planes[DRM_CAPTURE_DESTINATION_MAX_PLANES];
};

/*
 * Validate metadata shape and retained export write access without mapping,
 * waiting, acquiring buffer references or changing the description. The caller
 * supplies live buffers, not unchecked pointers copied from userspace.
 *
 * A provider must additionally validate the complete format/modifier layout,
 * row extents, allocation limits and supported exporter operations. A nonzero
 * stride and an offset inside the allocation do not prove the image fits.
 * Success grants no capture authority, reuse exclusion or pixel validity.
 */
int drm_capture_destination_validate(const struct drm_capture_destination *destination);

#endif
