// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <kunit/visibility.h>
#include <linux/uaccess.h>
#include <drm/drm_atomic.h>
#include <drm/drm_crtc.h>
#include <drm/drm_modeset_lock.h>
#include <drm/drm_property.h>

#include "drm_atomic_user_request.h"
#include "drm_crtc_internal.h"

int drm_atomic_initialize_user_fence_destinations(const struct drm_atomic_user_request *request)
{
	unsigned int i;

	for (i = 0; i < drm_atomic_user_request_fence_count(request); i++) {
		const struct drm_atomic_user_fence_destination *destination =
			drm_atomic_user_request_fence_destination(request, i);
		s32 __user *address = u64_to_user_ptr(destination->address);

		if (address && put_user(-1, address))
			return -EFAULT;
	}
	return 0;
}
EXPORT_SYMBOL_IF_KUNIT(drm_atomic_initialize_user_fence_destinations);

int drm_atomic_apply_user_fence_destinations(const struct drm_atomic_user_request *request,
					    struct drm_atomic_commit *state)
{
	unsigned int i;

	drm_modeset_lock_assert_held(&state->dev->mode_config.connection_mutex);
	for (i = 0; i < drm_atomic_user_request_fence_count(request); i++) {
		const struct drm_atomic_user_fence_destination *destination =
			drm_atomic_user_request_fence_destination(request, i);
		struct drm_crtc *crtc = destination->crtc;
		struct drm_property *property = destination->property;

		if (crtc->dev != state->dev || property != state->dev->mode_config.prop_out_fence_ptr ||
		    drm_mode_obj_find_prop_id(&crtc->base, property->base.id) != property)
			return -EINVAL;
	}
	for (i = 0; i < drm_atomic_user_request_fence_count(request); i++) {
		const struct drm_atomic_user_fence_destination *destination =
			drm_atomic_user_request_fence_destination(request, i);
		struct drm_crtc_state *crtc_state;

		crtc_state = drm_atomic_get_crtc_state(state, destination->crtc);
		if (IS_ERR(crtc_state))
			return PTR_ERR(crtc_state);
		if (destination->address)
			state->crtcs[drm_crtc_index(destination->crtc)].out_fence_ptr =
				u64_to_user_ptr(destination->address);
	}
	return 0;
}
EXPORT_SYMBOL_IF_KUNIT(drm_atomic_apply_user_fence_destinations);
