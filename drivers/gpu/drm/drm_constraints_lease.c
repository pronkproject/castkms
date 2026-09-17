// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <drm/drm_auth.h>
#include <drm/drm_constraints_output.h>
#include <drm/drm_crtc.h>
#include <drm/drm_device.h>

#include "drm_constraints_internal.h"
#include "drm_constraints_lease.h"

int drm_constraints_lease_acquire(struct drm_master *master, struct idr *leases)
{
	struct drm_device *dev = master->dev;
	struct drm_constraints_output *output;
	struct drm_crtc *crtc;
	u32 mask = 0;

	lockdep_assert_held(&dev->mode_config.idr_mutex);
	if (master->constraints_lease_crtcs)
		return -EINVAL;
	if (!dev->mode_config.constraints_domain)
		return 0;
	drm_for_each_crtc(crtc, dev) {
		output = crtc->constraints_output;
		if (!output || !idr_find(leases, crtc->base.id))
			continue;
		drm_modeset_lock_assert_held(&crtc->mutex);
		if (!crtc->state || crtc->state->constraints != output->default_entry)
			return -EBUSY;
		if ((u32)atomic_read(&output->leases) == U32_MAX)
			return -EOVERFLOW;
		mask |= drm_crtc_mask(crtc);
	}
	drm_for_each_crtc(crtc, dev)
		if (mask & drm_crtc_mask(crtc))
			atomic_inc(&crtc->constraints_output->leases);
	master->constraints_lease_crtcs = mask;
	return 0;
}

void drm_constraints_lease_release(struct drm_master *master)
{
	struct drm_device *dev = master->dev;
	struct drm_crtc *crtc;
	u32 mask = master->constraints_lease_crtcs;

	lockdep_assert_held(&dev->mode_config.idr_mutex);
	if (!mask)
		return;
	drm_for_each_crtc(crtc, dev)
		if (mask & drm_crtc_mask(crtc)) {
			if (WARN_ON_ONCE(!crtc->constraints_output ||
					 !atomic_read(&crtc->constraints_output->leases)))
				continue;
			atomic_dec(&crtc->constraints_output->leases);
		}
	master->constraints_lease_crtcs = 0;
}

int drm_constraints_lease_check(const struct drm_crtc_state *state)
{
	struct drm_crtc *crtc = state->crtc;
	struct drm_constraints_output *output = crtc->constraints_output;

	drm_modeset_lock_assert_held(&crtc->mutex);
	if (!output || state->constraints == output->default_entry)
		return 0;
	/*
	 * Adds cannot race the held CRTC lock; concurrent removal only relaxes it.
	 * Do not acquire idr_mutex during acceptance: preparation revocation takes
	 * that mutex before the owner lock held by the acceptance continuation.
	 */
	return atomic_read(&output->leases) ? -EBUSY : 0;
}
