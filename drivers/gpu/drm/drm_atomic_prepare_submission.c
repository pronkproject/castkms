// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/export.h>
#include <linux/slab.h>
#include <drm/drm_atomic.h>
#include <drm/drm_atomic_prepare_commit.h>
#include <drm/drm_atomic_prepare_display.h>
#include <drm/drm_atomic_prepare_outputs.h>
#include <drm/drm_atomic_prepare_owner.h>
#include <drm/drm_atomic_prepare_submission.h>
#include <drm/drm_atomic_prepare_ticket.h>

struct drm_atomic_prepare_submission {
	struct drm_prepare_owner *owner;
	struct drm_prepare_ticket *ticket;
	u32 crtc_mask;
};

int drm_atomic_prepare_submission_init(struct drm_atomic_commit *state,
				       struct drm_prepare_owner *owner)
{
	struct drm_atomic_prepare_submission *submission;

	if (state->prepare_submission || !owner)
		return -EINVAL;
	submission = kzalloc_obj(*submission);
	if (!submission)
		return -ENOMEM;
	submission->owner = drm_prepare_owner_get(owner);
	state->prepare_submission = submission;
	return 0;
}
EXPORT_SYMBOL_GPL(drm_atomic_prepare_submission_init);

int drm_atomic_prepare_submission_set(struct drm_atomic_commit *state,
				      struct drm_crtc *crtc,
				      struct drm_prepare_ticket *ticket)
{
	struct drm_atomic_prepare_submission *submission = state->prepare_submission;
	u32 mask;

	if (!submission)
		return -EOPNOTSUPP;
	if (crtc->dev != state->dev || !drm_atomic_get_new_crtc_state(state, crtc))
		return -EINVAL;
	mask = drm_crtc_mask(crtc);
	if (submission->crtc_mask & mask)
		return -EINVAL;
	if (!ticket)
		return 0;
	if (!drm_prepare_ticket_is_owned_by(ticket, submission->owner))
		return -EACCES;
	if (submission->ticket && submission->ticket != ticket)
		return -EINVAL;
	if (!submission->ticket)
		submission->ticket = drm_prepare_ticket_get(ticket);
	submission->crtc_mask |= mask;
	return 0;
}
EXPORT_SYMBOL_GPL(drm_atomic_prepare_submission_set);

static int observe_submission(struct drm_atomic_commit *state,
			      struct drm_prepare_output_generation *entries,
			      unsigned int capacity)
{
	struct drm_crtc *crtc;
	struct drm_crtc_state *old;
	u32 mask = 0;
	int i;

	for_each_old_crtc_in_state(state, crtc, old, i)
		mask |= drm_crtc_mask(crtc);
	if (mask != state->prepare_submission->crtc_mask)
		return -ESTALE;
	return drm_atomic_prepare_display_observe(state, entries, capacity);
}

int drm_atomic_prepare_submission_attach(struct drm_atomic_commit *state)
{
	struct drm_atomic_prepare_submission *submission = state->prepare_submission;

	if (!submission)
		return 0;
	if (!submission->ticket)
		return -EINVAL;
	return drm_atomic_commit_prepare_owned(state, submission->ticket, submission->owner,
					      observe_submission);
}
EXPORT_SYMBOL_GPL(drm_atomic_prepare_submission_attach);

void drm_atomic_prepare_submission_clear(struct drm_atomic_commit *state)
{
	struct drm_atomic_prepare_submission *submission = state->prepare_submission;

	if (!submission)
		return;
	state->prepare_submission = NULL;
	if (submission->ticket)
		drm_prepare_ticket_put(submission->ticket);
	drm_prepare_owner_put(submission->owner);
	kfree(submission);
}
