// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/dma-buf.h>
#include <linux/fs.h>
#include <linux/module.h>

#include <drm/drm_capture_destination.h>
#include <drm/drm_fourcc.h>

int drm_capture_destination_validate(const struct drm_capture_destination *destination)
{
	unsigned int i;

	if (!destination || !destination->width || !destination->height ||
	    !destination->format || destination->modifier == DRM_FORMAT_MOD_INVALID ||
	    !destination->num_planes ||
	    destination->num_planes > DRM_CAPTURE_DESTINATION_MAX_PLANES)
		return -EINVAL;

	for (i = 0; i < destination->num_planes; i++) {
		const struct drm_capture_destination_plane *plane = &destination->planes[i];

		if (!plane->buffer || !plane->stride || plane->offset >= plane->buffer->size)
			return -EINVAL;
		if (!(READ_ONCE(plane->buffer->file->f_mode) & FMODE_WRITE))
			return -EACCES;
	}
	return 0;
}
EXPORT_SYMBOL_GPL(drm_capture_destination_validate);
