// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <drm/drm_auth.h>
#include <drm/drm_constraints_output.h>
#include <drm/drm_crtc.h>
#include <drm/drm_file.h>
#include <drm/drm_lease.h>

#include "drm_constraints_client.h"
#include "drm_constraints_events.h"

static bool has_nondefault_selection(struct drm_file *file)
{
	struct drm_crtc *crtc;

	if (!drm_is_current_master(file))
		return false;
	drm_for_each_crtc(crtc, file->minor->dev) {
		if (!crtc->constraints_output || !drm_lease_held(file, crtc->base.id))
			continue;
		drm_modeset_lock_assert_held(&crtc->mutex);
		if (!crtc->state || crtc->state->constraints != drm_constraints_crtc_default(crtc))
			return true;
	}
	return false;
}

int drm_constraints_client_cap(struct drm_file *file, u64 value)
{
	struct drm_device *dev = file->minor->dev;
	struct drm_constraints_events *events;
	struct drm_modeset_acquire_ctx ctx;
	int ret;

	if (value > 1)
		return -EINVAL;
	if (!dev->mode_config.constraints_domain)
		return -EOPNOTSUPP;
	guard(mutex)(&file->constraints_lock);
	if (value && !file->atomic)
		return -EOPNOTSUPP;
	drm_modeset_acquire_init(&ctx, DRM_MODESET_ACQUIRE_INTERRUPTIBLE);
	for (;;) {
		ret = drm_modeset_lock_all_ctx(dev, &ctx);
		if (ret != -EDEADLK)
			break;
		ret = drm_modeset_backoff(&ctx);
		if (ret)
			break;
	}
	if (ret)
		goto unlock;
	if (!value && file->kms_constraints && has_nondefault_selection(file)) {
		ret = -EBUSY;
		goto unlock;
	}
	if (value && !file->constraints_events) {
		events = drm_constraints_events_create(file);
		if (IS_ERR(events)) {
			ret = PTR_ERR(events);
			goto unlock;
		}
		file->constraints_events = events;
	}
	WRITE_ONCE(file->kms_constraints, value);
unlock:
	drm_modeset_drop_locks(&ctx);
	drm_modeset_acquire_fini(&ctx);
	/* No event can be produced by a failed first subscription. */
	if (!ret && file->constraints_events) {
		if (value)
			drm_constraints_events_start(file->constraints_events);
		else
			drm_constraints_events_pause(file->constraints_events);
	}
	return ret;
}

void drm_constraints_client_release(struct drm_file *file)
{
	struct drm_constraints_events *events = file->constraints_events;

	file->constraints_events = NULL;
	drm_constraints_events_destroy(events);
}
