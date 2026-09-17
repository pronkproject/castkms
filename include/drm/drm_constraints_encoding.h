/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_CONSTRAINTS_ENCODING_H__
#define __DRM_CONSTRAINTS_ENCODING_H__

#include <linux/types.h>
#include <uapi/drm/drm_constraints.h>

struct drm_constraints_snapshot;

/*
 * Encode one retained immutable snapshot into a kernel buffer. NULL/zero is
 * size discovery. ENOSPC reports required bytes without modifying any payload;
 * other errors leave required unchanged. No alignment of buffer is required;
 * required and the retained snapshot must not overlap the output buffer.
 * Success writes exactly required bytes, with no references or authority in
 * the encoding. Snapshot ownership and expected-generation checking belong to
 * the list; copyout, client authority and notification delivery are separate.
 * Readers must reject unsupported versions and entries with unknown required
 * records. Per-plane format records are alternatives; scalar rules all apply.
 */
int drm_constraints_snapshot_encode(const struct drm_constraints_snapshot *snapshot,
				     void *buffer, size_t capacity, size_t *required);

#endif
