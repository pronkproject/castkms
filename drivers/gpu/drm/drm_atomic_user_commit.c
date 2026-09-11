// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <kunit/visibility.h>
#include <drm/drm_atomic.h>
#include <drm/drm_atomic_prepare_request.h>
#include <drm/drm_atomic_request.h>
#include <drm/drm_file.h>
#include <drm/drm_modeset_lock.h>

#include "drm_atomic_user_commit.h"
#include "drm_atomic_user_request.h"
#include "drm_atomic_user_signaling.h"

struct user_commit {
	struct drm_atomic_user_request *request;
	struct drm_atomic_user_signaling *signaling;
	struct drm_file *file;
	u32 flags;
	u32 event_crtcs;
	u64 user_data;
	bool plane_color_pipeline;
};

static int validate_request(struct drm_atomic_commit *state,
			    const struct drm_atomic_request *request, void *data)
{
	struct user_commit *commit = data;

	return drm_atomic_validate_user_request(commit->request, commit->file);
}

static int build_request(struct drm_atomic_commit *state, void *data)
{
	struct user_commit *commit = data;
	struct drm_crtc *crtc;
	struct drm_crtc_state *crtc_state;
	int i, ret;

	state->allow_modeset = !!(commit->flags & DRM_MODE_ATOMIC_ALLOW_MODESET);
	state->plane_color_pipeline = commit->plane_color_pipeline;
	ret = drm_atomic_request_apply(drm_atomic_user_request_values(commit->request),
				       state, validate_request, commit);
	if (ret)
		return ret;
	ret = drm_atomic_apply_user_fence_destinations(commit->request, state);
	if (ret)
		return ret;
	commit->event_crtcs = 0;
	for_each_new_crtc_in_state(state, crtc, crtc_state, i)
		commit->event_crtcs |= drm_crtc_mask(crtc);
	return 0;
}

static int prepare_signaling(struct drm_atomic_commit *state, void *data)
{
	struct user_commit *commit = data;

	return drm_atomic_prepare_user_signaling_for_crtcs(state, commit->file, commit->flags,
							  commit->user_data, commit->event_crtcs,
							  &commit->signaling);
}

static void complete_signaling(struct drm_atomic_commit *state, bool accepted, void *data)
{
	struct user_commit *commit = data;

	drm_atomic_complete_user_signaling(state, commit->signaling, accepted);
	commit->signaling = NULL;
}

static const struct drm_atomic_request_callbacks callbacks = {
	.build = build_request,
	.prepare_signaling = prepare_signaling,
	.complete_signaling = complete_signaling,
};

int drm_atomic_commit_user_request(struct drm_device *dev, struct drm_file *file,
				   struct drm_prepare_owner *owner, u32 flags, u64 user_data,
				   const struct drm_atomic_user_input *input)
{
	struct user_commit commit = {
		.file = file, .flags = flags, .user_data = user_data,
	};
	struct drm_modeset_acquire_ctx ctx;
	int ret;

	if (!file || !owner || file->minor->dev != dev ||
	    flags & ~(DRM_MODE_ATOMIC_ALLOW_MODESET | DRM_MODE_PAGE_FLIP_EVENT))
		return -EINVAL;
	if (!dev->mode_config.preparation)
		return -EOPNOTSUPP;
	commit.plane_color_pipeline = file->plane_color_pipeline;
	drm_modeset_acquire_init(&ctx, DRM_MODESET_ACQUIRE_INTERRUPTIBLE);
	for (;;) {
		ret = drm_modeset_lock_all_ctx(dev, &ctx);
		if (ret != -EDEADLK)
			break;
		ret = drm_modeset_backoff(&ctx);
		if (ret)
			break;
	}
	if (!ret) {
		commit.request = drm_atomic_resolve_user_request(dev, file, input);
		if (IS_ERR(commit.request))
			ret = PTR_ERR(commit.request);
	}
	drm_modeset_drop_locks(&ctx);
	drm_modeset_acquire_fini(&ctx);
	if (ret)
		return ret;
	ret = drm_atomic_initialize_user_fence_destinations(commit.request);
	if (!ret)
		ret = drm_atomic_commit_request_with_callbacks(dev, owner, &callbacks, &commit);
	drm_atomic_free_user_request(commit.request);
	return ret;
}
EXPORT_SYMBOL_IF_KUNIT(drm_atomic_commit_user_request);
