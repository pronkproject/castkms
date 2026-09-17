/* SPDX-License-Identifier: GPL-2.0 OR MIT */
#ifndef __DRM_LEASE_INTERNAL_H__
#define __DRM_LEASE_INTERNAL_H__

#include <kunit/visibility.h>

struct drm_master;
struct idr;

#if IS_ENABLED(CONFIG_KUNIT)
/* Native lease creation consumes the IDR only on success. */
struct drm_master *drm_lease_create(struct drm_master *lessor, struct idr *leases);
#endif

#endif
