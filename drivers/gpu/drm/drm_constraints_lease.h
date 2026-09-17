/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_CONSTRAINTS_LEASE_H__
#define __DRM_CONSTRAINTS_LEASE_H__

struct drm_crtc_state;
struct drm_master;
struct idr;

/* Acquire under all modeset locks and idr_mutex; failure changes no counters. */
int drm_constraints_lease_acquire(struct drm_master *master, struct idr *leases);
/* Release exactly the mask charged to this master, under idr_mutex. */
void drm_constraints_lease_release(struct drm_master *master);
/* Check a proposed binding under its CRTC lock, before taking the list lock. */
int drm_constraints_lease_check(const struct drm_crtc_state *state);

#endif
