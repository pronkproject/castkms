// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/err.h>
#include <linux/slab.h>
#include <drm/drm_atomic.h>
#include <drm/drm_atomic_constraints.h>
#include <drm/drm_atomic_prepare_request.h>
#include <drm/drm_atomic_uapi.h>
#include <drm/drm_constraints.h>
#include <drm/drm_constraints_list.h>
#include <drm/drm_constraints_entry.h>
#include <drm/drm_constraints_output.h>
#include <drm/drm_crtc.h>
#include <drm/drm_framebuffer.h>
#include <drm/drm_gem.h>
#include <drm/drm_fourcc.h>

#include "drm_constraints_internal.h"
#include "drm_constraints_lease.h"
#include "drm_crtc_internal.h"

static int candidate_available(struct drm_constraints_entry *entry, void *data)
{
	return 0;
}

struct drm_constraints_entry *
drm_atomic_resolve_constraints_for_crtc(struct drm_crtc *crtc, u64 id)
{
	struct drm_constraints_list *list;
	struct drm_constraints_entry *accepted;

	if (!crtc || !id)
		return ERR_PTR(-EINVAL);
	drm_modeset_lock_assert_held(&crtc->mutex);
	list = drm_constraints_crtc_list(crtc);
	if (!list)
		return ERR_PTR(-EOPNOTSUPP);
	accepted = crtc->state ? crtc->state->constraints : NULL;
	if (accepted && drm_constraints_entry_id(accepted) == id)
		return drm_constraints_entry_get(accepted);
	return drm_constraints_list_lookup(list, id);
}
EXPORT_SYMBOL_GPL(drm_atomic_resolve_constraints_for_crtc);

int drm_atomic_set_constraints_for_crtc(struct drm_crtc_state *state,
				       struct drm_constraints_entry *entry)
{
	struct drm_constraints_entry *old;
	struct drm_constraints_list *list;
	int ret;

	if (!entry || !state || !state->crtc)
		return -EINVAL;
	drm_modeset_lock_assert_held(&state->crtc->mutex);
	if (!state->state || state->state->dev != state->crtc->dev ||
	    drm_atomic_get_new_crtc_state(state->state, state->crtc) != state)
		return -EINVAL;
	if (state->state->checked)
		return -EBUSY;
	list = drm_constraints_crtc_list(state->crtc);
	if (!list)
		return -EOPNOTSUPP;
	if (entry == state->constraints)
		return 0;
	if (!state->crtc->state || entry != state->crtc->state->constraints) {
		ret = drm_constraints_list_check(list, entry, candidate_available, NULL);
		if (ret)
			return ret;
	}
	old = state->constraints;
	state->constraints = drm_constraints_entry_get(entry);
	if (old)
		drm_constraints_entry_put(old);
	return 0;
}
EXPORT_SYMBOL_GPL(drm_atomic_set_constraints_for_crtc);

static int build_restore_default(struct drm_atomic_commit *state, void *data)
{
	struct drm_crtc *crtc = data;
	struct drm_constraints_entry *entry = drm_constraints_crtc_default(crtc);
	struct drm_crtc_state *proposed, *old;
	int ret;

	if (!entry)
		return -EOPNOTSUPP;
	proposed = drm_atomic_get_crtc_state(state, crtc);
	if (IS_ERR(proposed))
		return PTR_ERR(proposed);
	old = drm_atomic_get_old_crtc_state(state, crtc);
	if (old->enable || old->active || old->plane_mask)
		return -EBUSY;
	/* Restoring a default is not shutdown through an unavailable binding. */
	ret = drm_constraints_list_check(drm_constraints_crtc_list(crtc), entry,
					 candidate_available, NULL);
	if (ret)
		return ret;
	ret = drm_atomic_set_constraints_for_crtc(proposed, entry);
	if (ret)
		return ret;
	return old->constraints == entry ? DRM_ATOMIC_REQUEST_UNCHANGED : 0;
}

int drm_atomic_constraints_restore_default(struct drm_crtc *crtc)
{
	return drm_atomic_commit_request(crtc->dev, build_restore_default, crtc);
}
EXPORT_SYMBOL_GPL(drm_atomic_constraints_restore_default);

static bool unchanged_disable(struct drm_atomic_commit *state, struct drm_crtc_state *crtc)
{
	const struct drm_crtc_state *old = drm_atomic_get_old_crtc_state(state, crtc->crtc);

	return old && !crtc->enable && !crtc->active && !crtc->plane_mask &&
		crtc->constraints == old->constraints;
}

static int find_output(struct drm_atomic_commit *state, struct drm_crtc_state **selected,
		       bool *quiesce_all)
{
	struct drm_crtc_state *crtc_state;
	struct drm_crtc *crtc;
	bool all_disabled = true;
	int i;

	*selected = NULL;
	*quiesce_all = false;
	for_each_new_crtc_in_state(state, crtc, crtc_state, i) {
		if (crtc->dev != state->dev || crtc_state->crtc != crtc)
			return -EXDEV;
		all_disabled &= unchanged_disable(state, crtc_state);
		if (!crtc->constraints_output) {
			if (crtc_state->constraints)
				return -EINVAL;
			continue;
		}
		if (!crtc_state->constraints)
			return -EINVAL;
		drm_modeset_lock_assert_held(&crtc->mutex);
		*selected = crtc_state;
	}
	if (*selected && state->async_update)
		return -EOPNOTSUPP;
	if (*selected && all_disabled)
		*quiesce_all = true;
	return 0;
}

int drm_atomic_constraints_prepare(struct drm_atomic_commit *state)
{
	struct drm_crtc_state *selected, *old;
	struct drm_crtc *crtc;
	bool quiesce_all;
	int i, ret;

	ret = find_output(state, &selected, &quiesce_all);
	if (ret || !selected)
		return ret;
	for_each_new_crtc_in_state(state, crtc, selected, i) {
		if (!crtc->constraints_output)
			continue;
		old = drm_atomic_get_old_crtc_state(state, crtc);
		if (selected->constraints != old->constraints) {
			if (!state->allow_modeset)
				return -EINVAL;
			selected->mode_changed = true;
		}
		ret = drm_atomic_add_affected_planes(state, crtc);
		if (ret)
			return ret;
	}
	return 0;
}
EXPORT_SYMBOL_GPL(drm_atomic_constraints_prepare);

static bool size_matches(const struct drm_constraints_size *size, u32 width, u32 height)
{
	return width >= size->min_width && width <= size->max_width &&
		height >= size->min_height && height <= size->max_height;
}

static bool storage_matches(const struct drm_constraints_format *format,
			    const struct drm_framebuffer *fb)
{
	unsigned int i;

	for (i = 0; i < fb->format->num_planes; i++) {
		bool imported = fb->obj[i] && drm_gem_is_imported(fb->obj[i]);
		u32 origin = imported ? DRM_CONSTRAINTS_FORMAT_STORAGE_IMPORTED :
					DRM_CONSTRAINTS_FORMAT_STORAGE_NATIVE;

		if (!(format->storage_flags & origin) ||
		    !IS_ALIGNED(fb->pitches[i], format->pitch_alignment) ||
		    !IS_ALIGNED(fb->offsets[i], format->offset_alignment) ||
		    fb->pitches[i] > format->max_pitch)
			return false;
	}
	return true;
}

struct constraints_update {
	struct drm_atomic_commit *state;
	struct drm_crtc_state *crtc;
	void (*install)(struct drm_atomic_commit *state);
};

static bool quiescing_output(const struct constraints_update *update)
{
	return unchanged_disable(update->state, update->crtc);
}

static int check_properties(struct constraints_update *update,
			    struct drm_constraints_description *description)
{
	const struct drm_constraints_property *rules;
	struct drm_plane_state *plane_state;
	struct drm_plane *plane;
	unsigned int count, i;
	int j, ret;

	if (!update->crtc->enable)
		return 0;
	rules = drm_constraints_description_properties(description, &count);
	for (i = 0; i < count; i++) {
		struct drm_mode_object *object = NULL;
		struct drm_property *property;
		struct drm_plane_state *rule_plane_state = NULL;
		u64 value;

		if (rules[i].object_id == update->crtc->crtc->base.id) {
			object = &update->crtc->crtc->base;
		} else {
			for_each_new_plane_in_state(update->state, plane, plane_state, j) {
				if (plane->base.id == rules[i].object_id &&
				    plane_state->crtc == update->crtc->crtc) {
					object = &plane->base;
					rule_plane_state = plane_state;
					break;
				}
			}
		}
		if (!object)
			continue;
		if ((rules[i].flags & DRM_CONSTRAINTS_PROPERTY_PLANE_YUV) &&
		    (!rule_plane_state || !rule_plane_state->fb ||
		     !rule_plane_state->fb->format->is_yuv))
			continue;
		property = drm_mode_obj_find_prop_id(object, rules[i].property_id);
		ret = drm_atomic_get_property_from_state(update->state, object, property, &value);
		if (ret)
			return ret;
		if (!drm_constraints_property_matches(&rules[i], value))
			return -EINVAL;
	}
	return 0;
}

static int check_plane_limits(struct constraints_update *update,
			      struct drm_constraints_description *description)
{
	const struct drm_constraints_plane_limit *limits;
	struct drm_plane_state *plane_state;
	struct drm_plane *plane;
	unsigned int count, active, i, id;
	int state_index;

	limits = drm_constraints_description_plane_limits(description, &count);
	for (i = 0; i < count; i++) {
		active = 0;
		for_each_new_plane_in_state(update->state, plane, plane_state, state_index) {
			if (plane_state->crtc != update->crtc->crtc)
				continue;
			for (id = 0; id < limits[i].count; id++)
				if (limits[i].plane_ids[id] == plane->base.id) {
					active++;
					break;
				}
		}
		if (active > limits[i].max_active)
			return -EINVAL;
	}
	return 0;
}

static int check_plane_geometry(struct drm_constraints_description *description,
				struct drm_plane *plane,
				const struct drm_plane_state *state)
{
	const struct drm_constraints_plane_geometry *geometries;
	const struct drm_constraints_plane_geometry *geometry = NULL;
	u64 framebuffer_extent, source_extent, destination_extent;
	unsigned int count, i, axis;

	geometries = drm_constraints_description_plane_geometries(description, &count);
	for (i = 0; i < count; i++)
		if (geometries[i].plane_id == plane->base.id) {
			geometry = &geometries[i];
			break;
		}
	if (!geometry)
		return 0;
	if (!(geometry->flags & DRM_CONSTRAINTS_GEOMETRY_POSITION) &&
	    (state->crtc_x || state->crtc_y))
		return -EINVAL;
	if (!(geometry->flags & DRM_CONSTRAINTS_GEOMETRY_FRACTIONAL_SOURCE) &&
	    ((state->src_x | state->src_y | state->src_w | state->src_h) & 0xffff))
		return -EINVAL;
	for (axis = 0; axis < 2; axis++) {
		u64 source_origin = axis ? state->src_y : state->src_x;

		source_extent = axis ? state->src_h : state->src_w;
		destination_extent = axis ? state->crtc_h : state->crtc_w;
		framebuffer_extent = (u64)(axis ? state->fb->height : state->fb->width) << 16;
		if (!source_extent || !destination_extent ||
		    source_origin + source_extent > framebuffer_extent ||
		    (!(geometry->flags & DRM_CONSTRAINTS_GEOMETRY_CROP) &&
		     (source_origin || source_extent != framebuffer_extent)) ||
		    source_extent < destination_extent * geometry->min_scale ||
		    source_extent > destination_extent * geometry->max_scale)
			return -EINVAL;
	}
	return 0;
}

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
	int i, ret;

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
		bool implicit;

		if (plane_state->crtc != update->crtc->crtc)
			continue;
		if (!fb)
			return -EINVAL;
		implicit = !(fb->flags & DRM_MODE_FB_MODIFIERS);
		for (j = 0; j < count; j++) {
			if (implicit == !!(formats[j].flags & DRM_CONSTRAINTS_FORMAT_IMPLICIT) &&
			    formats[j].plane_id == plane->base.id &&
			    formats[j].format == fb->format->format &&
			    (implicit || formats[j].modifier == fb->modifier) &&
			    size_matches(&formats[j].size, fb->width, fb->height) &&
			    IS_ALIGNED(fb->width, formats[j].width_alignment) &&
			    IS_ALIGNED(fb->height, formats[j].height_alignment) &&
			    storage_matches(&formats[j], fb))
				break;
		}
		if (j == count)
			return -EINVAL;
		ret = check_plane_geometry(description, plane, plane_state);
		if (ret)
			return ret;
		mask |= drm_plane_mask(plane);
	}
	if (mask != update->crtc->plane_mask)
		return -EINVAL;
	ret = check_plane_limits(update, description);
	if (ret)
		return ret;
	/* Stopping scanout requires no new work from an unavailable backend. */
	if (quiescing_output(update))
		return 0;
	ret = check_properties(update, description);
	if (ret)
		return ret;
	return output->ops->check(update->state, update->crtc, entry);
}

static int check_quiescing_outputs(struct drm_atomic_commit *state)
{
	struct constraints_update update = { .state = state };
	struct drm_crtc *crtc;
	int i, ret;

	for_each_new_crtc_in_state(state, crtc, update.crtc, i) {
		if (!crtc->constraints_output)
			continue;
		ret = drm_constraints_list_quiesce(drm_constraints_crtc_list(crtc),
						    update.crtc->constraints, check_scene, &update);
		if (ret)
			return ret;
	}
	return 0;
}

int drm_atomic_constraints_check(struct drm_atomic_commit *state)
{
	struct constraints_update update = { .state = state };
	struct drm_constraints_list *list;
	struct drm_crtc *crtc;
	bool quiesce_all;
	int i;
	int ret = find_output(state, &update.crtc, &quiesce_all);

	if (ret || !update.crtc)
		return ret;
	if (quiesce_all)
		return check_quiescing_outputs(state);
	for_each_new_crtc_in_state(state, crtc, update.crtc, i) {
		list = drm_constraints_crtc_list(crtc);
		if (!list)
			continue;
		if (quiescing_output(&update)) {
			ret = drm_constraints_list_quiesce(list, update.crtc->constraints,
							  check_scene, &update);
		} else {
			ret = drm_constraints_lease_check(update.crtc);
			if (!ret)
				ret = drm_constraints_list_check(list, update.crtc->constraints,
							 check_scene, &update);
		}
		if (ret)
			return ret;
	}
	return 0;
}
EXPORT_SYMBOL_GPL(drm_atomic_constraints_check);

static int install_scenes(void *data)
{
	struct constraints_update *update = data;
	struct drm_crtc *crtc;
	int i, ret;

	for_each_new_crtc_in_state(update->state, crtc, update->crtc, i) {
		if (!crtc->constraints_output)
			continue;
		ret = check_scene(update->crtc->constraints, update);
		if (ret)
			return ret;
	}
	update->install(update->state);
	return 0;
}

int drm_atomic_constraints_install(struct drm_atomic_commit *state,
				    void (*install)(struct drm_atomic_commit *state))
{
	struct constraints_update update = { .state = state, .install = install };
	struct drm_constraints_selection *selections;
	struct drm_crtc *crtc;
	unsigned int count = 0;
	bool quiesce_all;
	int i, ret;

	if (!install)
		return -EINVAL;
	ret = find_output(state, &update.crtc, &quiesce_all);
	if (ret)
		return ret;
	if (!update.crtc) {
		install(state);
		return 0;
	}
	if (quiesce_all) {
		ret = check_quiescing_outputs(state);
		if (ret)
			return ret;
		/*
		 * Modeset locks stabilize every retained selection through swap.
		 * No offer is selected or backend work accepted, so withdrawal and
		 * closure may race without requiring nested list locks.
		 */
		install(state);
		return 0;
	}
	selections = kcalloc(state->dev->mode_config.num_crtc, sizeof(*selections), GFP_KERNEL);
	if (!selections)
		return -ENOMEM;
	for_each_new_crtc_in_state(state, crtc, update.crtc, i) {
		if (!crtc->constraints_output)
			continue;
		if (!quiescing_output(&update)) {
			ret = drm_constraints_lease_check(update.crtc);
			if (ret)
				goto out;
		}
		selections[count++] = (struct drm_constraints_selection) {
			.list = drm_constraints_crtc_list(crtc),
			.entry = update.crtc->constraints,
			.quiesce = quiescing_output(&update),
		};
	}
	ret = drm_constraints_lists_accept(selections, count, install_scenes, &update);
out:
	kfree(selections);
	return ret;
}
EXPORT_SYMBOL_GPL(drm_atomic_constraints_install);
