// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/err.h>
#include <drm/drm_atomic.h>
#include <drm/drm_atomic_constraints.h>
#include <drm/drm_constraints.h>
#include <drm/drm_constraints_catalog.h>
#include <drm/drm_constraints_entry.h>
#include <drm/drm_constraints_output.h>
#include <drm/drm_crtc.h>
#include <drm/drm_framebuffer.h>
#include <drm/drm_fourcc.h>

#include "drm_constraints_internal.h"

static int candidate_available(struct drm_constraints_entry *entry, void *data)
{
	return 0;
}

int drm_atomic_set_constraints_for_crtc(struct drm_crtc_state *state,
				       struct drm_constraints_entry *entry)
{
	struct drm_constraints_entry *old;
	struct drm_constraints_catalog *catalog;
	int ret;

	if (!entry || !state || !state->crtc)
		return -EINVAL;
	drm_modeset_lock_assert_held(&state->crtc->mutex);
	if (state->state && state->state->checked)
		return -EBUSY;
	catalog = drm_constraints_crtc_catalog(state->crtc);
	if (!catalog)
		return -EOPNOTSUPP;
	ret = drm_constraints_catalog_check(catalog, entry, candidate_available, NULL);
	if (ret)
		return ret;
	old = state->constraints;
	state->constraints = drm_constraints_entry_get(entry);
	if (old)
		drm_constraints_entry_put(old);
	return 0;
}
EXPORT_SYMBOL_GPL(drm_atomic_set_constraints_for_crtc);

static int find_output(struct drm_atomic_commit *state, struct drm_crtc_state **selected)
{
	struct drm_crtc_state *crtc_state;
	struct drm_crtc *crtc;
	unsigned int count = 0;
	int i;

	*selected = NULL;
	for_each_new_crtc_in_state(state, crtc, crtc_state, i) {
		count++;
		if (!crtc->constraints_output) {
			if (crtc_state->constraints)
				return -EINVAL;
			continue;
		}
		if (!crtc_state->constraints)
			return -EINVAL;
		if (crtc->dev != state->dev || crtc_state->crtc != crtc)
			return -EXDEV;
		*selected = crtc_state;
	}
	if (*selected && (count != 1 || state->async_update))
		return -EOPNOTSUPP;
	return 0;
}

int drm_atomic_constraints_prepare(struct drm_atomic_commit *state)
{
	struct drm_crtc_state *selected, *old;
	int ret;

	ret = find_output(state, &selected);
	if (ret || !selected)
		return ret;
	old = drm_atomic_get_old_crtc_state(state, selected->crtc);
	if (selected->constraints != old->constraints) {
		if (!state->allow_modeset)
			return -EINVAL;
		selected->mode_changed = true;
	}
	return drm_atomic_add_affected_planes(state, selected->crtc);
}
EXPORT_SYMBOL_GPL(drm_atomic_constraints_prepare);

static bool size_matches(const struct drm_constraints_size *size, u32 width, u32 height)
{
	return width >= size->min_width && width <= size->max_width &&
		height >= size->min_height && height <= size->max_height;
}

struct constraints_update {
	struct drm_atomic_commit *state;
	struct drm_crtc_state *crtc;
	void (*install)(struct drm_atomic_commit *state);
};

static int check_scene(struct drm_constraints_entry *entry, void *data)
{
	struct constraints_update *update = data;
	struct drm_constraints_description *description = drm_constraints_entry_description(entry);
	struct drm_constraints_output *output = update->crtc->crtc->constraints_output;
	const struct drm_constraints_format *formats;
	struct drm_plane_state *plane_state;
	struct drm_plane *plane;
	unsigned int count, j;
	u32 mask = 0;
	int i;

	if (!update->state->allow_modeset &&
	    entry != drm_atomic_get_old_crtc_state(update->state, update->crtc->crtc)->constraints)
		return -EINVAL;
	if (update->crtc->enable &&
	    (update->crtc->mode.hdisplay <= 0 || update->crtc->mode.vdisplay <= 0 ||
	     !size_matches(drm_constraints_description_output(description),
			   update->crtc->mode.hdisplay, update->crtc->mode.vdisplay)))
		return -EINVAL;
	formats = drm_constraints_description_formats(description, &count);
	for_each_new_plane_in_state(update->state, plane, plane_state, i) {
		struct drm_framebuffer *fb = plane_state->fb;

		if (plane_state->crtc != update->crtc->crtc)
			continue;
		if (!fb)
			return -EINVAL;
		for (j = 0; j < count; j++) {
			if (formats[j].plane_id == plane->base.id &&
			    formats[j].format == fb->format->format &&
			    formats[j].modifier == fb->modifier &&
			    size_matches(&formats[j].size, fb->width, fb->height))
				break;
		}
		if (j == count)
			return -EINVAL;
		mask |= drm_plane_mask(plane);
	}
	if (mask != update->crtc->plane_mask)
		return -EINVAL;
	return output->ops->check(update->state, update->crtc, drm_constraints_entry_data(entry));
}

int drm_atomic_constraints_check(struct drm_atomic_commit *state)
{
	struct constraints_update update = { .state = state };
	int ret = find_output(state, &update.crtc);

	if (ret || !update.crtc)
		return ret;
	return drm_constraints_catalog_check(drm_constraints_crtc_catalog(update.crtc->crtc),
					     update.crtc->constraints, check_scene, &update);
}
EXPORT_SYMBOL_GPL(drm_atomic_constraints_check);

static int install_scene(struct drm_constraints_entry *entry, void *data)
{
	struct constraints_update *update = data;
	int ret = check_scene(entry, data);

	if (ret)
		return ret;
	update->install(update->state);
	return 0;
}

int drm_atomic_constraints_install(struct drm_atomic_commit *state,
				    void (*install)(struct drm_atomic_commit *state))
{
	struct constraints_update update = { .state = state, .install = install };
	int ret;

	if (!install)
		return -EINVAL;
	ret = find_output(state, &update.crtc);
	if (ret)
		return ret;
	if (!update.crtc) {
		install(state);
		return 0;
	}
	return drm_constraints_catalog_accept(drm_constraints_crtc_catalog(update.crtc->crtc),
					      update.crtc->constraints, install_scene, &update);
}
EXPORT_SYMBOL_GPL(drm_atomic_constraints_install);
