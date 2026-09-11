// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <kunit/visibility.h>
#include <drm/drm_atomic.h>
#include <drm/drm_atomic_prepare_auth.h>
#include <drm/drm_atomic_prepare_owner.h>
#include <drm/drm_atomic_prepare_request.h>
#include <drm/drm_auth.h>
#include <drm/drm_framebuffer.h>
#include <drm/drm_lease.h>

#include "drm_atomic_user_commit.h"
#include "drm_atomic_user_signaling.h"

struct user_flip {
	struct drm_file *file;
	struct drm_crtc *crtc;
	struct drm_framebuffer *fb;
	struct drm_atomic_user_signaling *signaling;
	u64 user_data;
	bool event;
};

static int build_flip(struct drm_atomic_commit *state, void *data)
{
	struct user_flip *request = data;
	struct drm_crtc *crtc = request->crtc;
	int ret;

	ret = drm_modeset_lock_all_ctx(state->dev, state->acquire_ctx);
	if (ret)
		return ret;
	if (!drm_is_current_master(request->file) ||
	    !drm_lease_held(request->file, crtc->base.id) ||
	    !drm_lease_held(request->file, crtc->primary->base.id))
		return -EACCES;
	return crtc->funcs->build_page_flip(state, crtc, request->fb);
}

static int prepare_event(struct drm_atomic_commit *state, void *data)
{
	struct user_flip *request = data;

	if (!request->event)
		return 0;
	return drm_atomic_prepare_user_flip_event(state, request->crtc, request->file,
						 request->user_data, &request->signaling);
}

static void complete_event(struct drm_atomic_commit *state, bool accepted, void *data)
{
	struct user_flip *request = data;

	drm_atomic_complete_user_signaling(state, request->signaling, accepted);
	request->signaling = NULL;
}

int drm_atomic_submit_user_flip(struct drm_crtc *crtc,
				const struct drm_mode_crtc_page_flip_target *input,
				struct drm_file *file)
{
	static const struct drm_atomic_request_callbacks callbacks = {
		.build = build_flip,
		.prepare_signaling = prepare_event,
		.complete_signaling = complete_event,
	};
	struct user_flip request = {
		.file = file,
		.crtc = crtc,
		.user_data = input->user_data,
		.event = input->flags & DRM_MODE_PAGE_FLIP_EVENT,
	};
	struct drm_prepare_owner *owner;
	int ret;

	if (!crtc->primary || !crtc->funcs->build_page_flip ||
	    input->flags & ~DRM_MODE_PAGE_FLIP_EVENT || input->sequence)
		return -EOPNOTSUPP;
	owner = drm_file_prepare_owner(file);
	if (IS_ERR(owner))
		return PTR_ERR(owner);
	request.fb = drm_framebuffer_lookup(crtc->dev, file, input->fb_id);
	if (!request.fb) {
		ret = -ENOENT;
		goto out;
	}
	ret = drm_atomic_submit_request_with_callbacks(crtc->dev, owner, &callbacks, &request);
	drm_framebuffer_put(request.fb);
out:
	drm_prepare_owner_put(owner);
	return ret;
}
EXPORT_SYMBOL_IF_KUNIT(drm_atomic_submit_user_flip);
