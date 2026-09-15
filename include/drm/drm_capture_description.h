/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_CAPTURE_DESCRIPTION_H__
#define __DRM_CAPTURE_DESCRIPTION_H__

#include <linux/types.h>

/**
 * struct drm_capture_description - one offered configuration of a final image
 * @id: Nonzero name within one capture client, not an authority or object address.
 * @width: Nonzero visible width in pixels.
 * @height: Nonzero visible height in pixels.
 * @refresh_millihz: Nonzero accepted display refresh rate in millihertz.
 * @mode_flags: DRM_MODE_FLAG_* values for the accepted display mode.
 * @format: Nonzero DRM fourcc format of the offered image.
 * @max_requests: Nonzero maximum requests per stream, not reserved queue credit.
 * @modifier: DRM format modifier, not DRM_FORMAT_MOD_INVALID.
 *
 * The provider retains the configuration represented by id until replacing the
 * offer or destroying its client. Names must not be reused within that client.
 * Repeating an unchanged offer preserves its name. Opening a stream must check
 * current permission and the named configuration again. Dimensions alone do
 * not identify a configuration, and a description does not authorize pixels.
 *
 * This is image metadata, not an allocation description: destination strides,
 * offsets, planes and exporter support need independent validation. Querying
 * an offer must not reserve image storage, claim a source or start rendering.
 */
struct drm_capture_description {
	u64 id;
	u32 width;
	u32 height;
	u32 refresh_millihz;
	u32 mode_flags;
	u32 format;
	u32 max_requests;
	u64 modifier;
};

#endif
