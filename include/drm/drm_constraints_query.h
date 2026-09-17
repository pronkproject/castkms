/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_CONSTRAINTS_QUERY_H__
#define __DRM_CONSTRAINTS_QUERY_H__

struct drm_constraints_list;
struct drm_mode_list_constraints;

/*
 * Copy a coherent list to userspace using the common request layout. The caller
 * retains the list and establishes client authority and output visibility.
 * No DRM lookup, client opt-in or ioctl dispatch is performed here. The request
 * is a kernel copy; its CRTC ID must match the supplied list. Success and ENOSPC
 * update generation/size; all other errors leave the request unchanged. EFAULT
 * may have copied part of the payload, which the caller must discard.
 *
 * May allocate and fault. Do not hold modeset, list or provider admission locks.
 */
int drm_constraints_list_copy_to_user(struct drm_constraints_list *list,
				      struct drm_mode_list_constraints *request);

#endif
