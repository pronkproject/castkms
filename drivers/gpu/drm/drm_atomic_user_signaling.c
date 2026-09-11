// SPDX-License-Identifier: MIT
/* Copyright (C) 2014, 2018 Intel Corp.
 * Copyright (C) 2014 Red Hat
 * Copyright (c) 2020, The Linux Foundation. All rights reserved.
 */

#include <kunit/visibility.h>
#include <linux/file.h>
#include <linux/slab.h>
#include <linux/sync_file.h>
#include <linux/uaccess.h>
#include <drm/drm_atomic.h>
#include <drm/drm_file.h>
#include <drm/drm_print.h>
#include <drm/drm_vblank.h>
#include <drm/drm_writeback.h>

#include "drm_atomic_user_signaling.h"
#include "drm_crtc_internal.h"

struct drm_atomic_user_fence {
	s32 __user *destination;
	struct sync_file *sync_file;
	int fd;
};

struct drm_atomic_user_signaling {
	struct drm_atomic_user_fence *fences;
	unsigned int fence_count;
	unsigned int event_count;
	struct drm_pending_vblank_event *events[] __counted_by(event_count);
};

static int prepare_fence(struct drm_atomic_user_signaling *signaling,
			 s32 __user *destination, struct dma_fence *fence)
{
	struct drm_atomic_user_fence *fences, *entry;

	fences = krealloc_array(signaling->fences, signaling->fence_count + 1,
				 sizeof(*fences), GFP_KERNEL);
	if (!fences)
		return -ENOMEM;
	signaling->fences = fences;
	entry = &fences[signaling->fence_count++];
	*entry = (struct drm_atomic_user_fence) { .destination = destination, .fd = -1 };
	entry->fd = get_unused_fd_flags(O_CLOEXEC);
	if (entry->fd < 0)
		return entry->fd;
	if (put_user(entry->fd, destination))
		return -EFAULT;
	entry->sync_file = sync_file_create(fence);
	return entry->sync_file ? 0 : -ENOMEM;
}

static int prepare_crtc(struct drm_atomic_commit *state, struct drm_crtc *crtc,
			struct drm_crtc_state *new_state, struct drm_file *file,
			u32 flags, u64 user_data, struct drm_atomic_user_signaling *signaling)
{
	unsigned int index = drm_crtc_index(crtc);
	s32 __user *destination = state->crtcs[index].out_fence_ptr;
	struct drm_crtc_state *old_state = drm_atomic_get_old_crtc_state(state, crtc);
	struct drm_pending_vblank_event *event;
	struct dma_fence *fence;
	int ret;

	state->crtcs[index].out_fence_ptr = NULL;
	if (!(flags & DRM_MODE_PAGE_FLIP_EVENT) && !destination)
		return 0;
	if (!new_state->active && !old_state->active)
		return -EINVAL;
	if (new_state->event)
		return -EBUSY;
	event = kzalloc_obj(*event);
	if (!event)
		return -ENOMEM;
	event->event.base.type = DRM_EVENT_FLIP_COMPLETE;
	event->event.base.length = sizeof(event->event);
	event->event.vbl.crtc_id = crtc->base.id;
	event->event.vbl.user_data = user_data;
	signaling->events[index] = event;
	new_state->event = event;
	if (flags & DRM_MODE_PAGE_FLIP_EVENT) {
		ret = drm_event_reserve_init(state->dev, file, &event->base, &event->event.base);
		if (ret)
			return ret;
	}
	if (!destination)
		return 0;
	fence = drm_crtc_create_fence(crtc);
	if (!fence)
		return -ENOMEM;
	ret = prepare_fence(signaling, destination, fence);
	if (ret) {
		dma_fence_put(fence);
		return ret;
	}
	event->base.fence = fence;
	return 0;
}

int drm_atomic_prepare_user_flip_event(struct drm_atomic_commit *state,
				       struct drm_crtc *crtc, struct drm_file *file,
				       u64 user_data, struct drm_atomic_user_signaling **result)
{
	struct drm_atomic_user_signaling *signaling;
	struct drm_crtc_state *new_state;

	if (*result || !file || !crtc || crtc->dev != state->dev)
		return -EINVAL;
	new_state = drm_atomic_get_new_crtc_state(state, crtc);
	if (!new_state || state->crtcs[drm_crtc_index(crtc)].out_fence_ptr)
		return -EINVAL;
	signaling = kzalloc(struct_size(signaling, events, state->dev->mode_config.num_crtc),
			    GFP_KERNEL);
	if (!signaling)
		return -ENOMEM;
	signaling->event_count = state->dev->mode_config.num_crtc;
	*result = signaling;
	return prepare_crtc(state, crtc, new_state, file, DRM_MODE_PAGE_FLIP_EVENT,
			    user_data, signaling);
}
EXPORT_SYMBOL_IF_KUNIT(drm_atomic_prepare_user_flip_event);

int drm_atomic_prepare_user_signaling_for_crtcs(struct drm_atomic_commit *state,
						struct drm_file *file, u32 flags, u64 user_data,
						u32 event_crtcs,
						struct drm_atomic_user_signaling **result)
{
	struct drm_atomic_user_signaling *signaling;
	struct drm_crtc *crtc;
	struct drm_crtc_state *crtc_state;
	struct drm_connector *connector;
	struct drm_connector_state *connector_state;
	unsigned int crtc_count = 0;
	int i, ret;

	if (*result || !file)
		return -EINVAL;
	if (flags & DRM_MODE_ATOMIC_TEST_ONLY)
		return 0;
	signaling = kzalloc(struct_size(signaling, events, state->dev->mode_config.num_crtc),
			    GFP_KERNEL);
	if (!signaling)
		return -ENOMEM;
	signaling->event_count = state->dev->mode_config.num_crtc;
	*result = signaling;
	for_each_new_crtc_in_state(state, crtc, crtc_state, i) {
		u32 crtc_flags = flags;

		if (event_crtcs & drm_crtc_mask(crtc))
			crtc_count++;
		else
			crtc_flags &= ~DRM_MODE_PAGE_FLIP_EVENT;
		ret = prepare_crtc(state, crtc, crtc_state, file, crtc_flags, user_data, signaling);
		if (ret)
			return ret;
	}
	if (!crtc_count && (flags & DRM_MODE_PAGE_FLIP_EVENT))
		return -EINVAL;
	for_each_new_connector_in_state(state, connector, connector_state, i) {
		unsigned int index = drm_connector_index(connector);
		s32 __user *destination = state->connectors[index].out_fence_ptr;
		struct dma_fence *fence;

		if (!connector_state->writeback_job || !destination)
			continue;
		state->connectors[index].out_fence_ptr = NULL;
		fence = drm_writeback_get_out_fence(drm_connector_to_writeback(connector));
		if (!fence)
			return -ENOMEM;
		ret = prepare_fence(signaling, destination, fence);
		if (ret) {
			dma_fence_put(fence);
			return ret;
		}
		connector_state->writeback_job->out_fence = fence;
	}
	return 0;
}

int drm_atomic_prepare_user_signaling(struct drm_atomic_commit *state,
				      struct drm_file *file, u32 flags, u64 user_data,
				      struct drm_atomic_user_signaling **result)
{
	struct drm_crtc *crtc;
	struct drm_crtc_state *crtc_state;
	u32 event_crtcs = 0;
	int i;

	for_each_new_crtc_in_state(state, crtc, crtc_state, i)
		event_crtcs |= drm_crtc_mask(crtc);
	return drm_atomic_prepare_user_signaling_for_crtcs(state, file, flags, user_data,
							  event_crtcs, result);
}
EXPORT_SYMBOL_IF_KUNIT(drm_atomic_prepare_user_signaling);

void drm_atomic_complete_user_signaling(struct drm_atomic_commit *state,
					struct drm_atomic_user_signaling *signaling,
					bool accepted)
{
	unsigned int i;

	if (!signaling)
		return;
	if (!accepted) {
		for (i = 0; i < signaling->event_count; i++) {
			struct drm_pending_vblank_event *event = signaling->events[i];

			if (!event)
				continue;
			if (state->crtcs[i].new_state->event == event)
				state->crtcs[i].new_state->event = NULL;
			drm_event_cancel_free(state->dev, &event->base);
		}
	}
	for (i = 0; i < signaling->fence_count; i++) {
		struct drm_atomic_user_fence *entry = &signaling->fences[i];

		if (accepted) {
			fd_install(entry->fd, entry->sync_file->file);
			continue;
		}
		if (entry->sync_file)
			fput(entry->sync_file->file);
		if (entry->fd >= 0)
			put_unused_fd(entry->fd);
		if (put_user(-1, entry->destination))
			drm_dbg_atomic(state->dev, "Couldn't clear output fence destination\n");
	}
	kfree(signaling->fences);
	kfree(signaling);
}
EXPORT_SYMBOL_IF_KUNIT(drm_atomic_complete_user_signaling);
