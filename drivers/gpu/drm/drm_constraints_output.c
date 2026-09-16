// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/err.h>
#include <linux/slab.h>
#include <drm/drm_constraints.h>
#include <drm/drm_constraints_catalog.h>
#include <drm/drm_constraints_entry.h>
#include <drm/drm_constraints_output.h>
#include <drm/drm_crtc.h>
#include <drm/drm_device.h>
#include <drm/drm_drv.h>

#include "drm_constraints_internal.h"

static int validate_scope(struct drm_crtc *crtc, struct drm_constraints_entry *entry)
{
	const struct drm_constraints_format *formats;
	struct drm_plane *plane;
	unsigned int count, i;
	bool found;

	if (!entry || !drm_constraints_entry_in_domain(entry, crtc->dev->mode_config.constraints_domain) ||
	    drm_constraints_entry_crtc(entry) != crtc->base.id)
		return -EINVAL;
	formats = drm_constraints_description_formats(drm_constraints_entry_description(entry), &count);
	for (i = 0; i < count; i++) {
		found = false;
		drm_for_each_plane(plane, crtc->dev) {
			if (plane->base.id == formats[i].plane_id &&
			    (plane->possible_crtcs & drm_crtc_mask(crtc))) {
				found = true;
				break;
			}
		}
		if (!found)
			return -EINVAL;
	}
	return 0;
}

int drm_constraints_crtc_init(struct drm_crtc *crtc, struct drm_constraints_entry *initial,
			      unsigned int limit, const struct drm_constraints_output_ops *ops)
{
	struct drm_constraints_output *output;
	int ret;

	if (!crtc->dev->mode_config.constraints_domain || !ops || !ops->check)
		return -EINVAL;
	if (crtc->dev->registered || crtc->constraints_output ||
	    (crtc->state && (crtc->state->enable || crtc->state->active ||
			     crtc->state->commit || crtc->state->constraints)))
		return -EBUSY;
	ret = validate_scope(crtc, initial);
	if (ret)
		return ret;
	output = kzalloc_obj(*output);
	if (!output)
		return -ENOMEM;
	output->catalog = drm_constraints_catalog_create(crtc->dev->mode_config.constraints_domain,
							 initial, limit);
	if (IS_ERR(output->catalog)) {
		ret = PTR_ERR(output->catalog);
		kfree(output);
		return ret;
	}
	output->ops = ops;
	crtc->constraints_output = output;
	if (crtc->state)
		crtc->state->constraints = drm_constraints_entry_get(initial);
	return 0;
}
EXPORT_SYMBOL_GPL(drm_constraints_crtc_init);

void drm_constraints_crtc_fini(struct drm_crtc *crtc)
{
	struct drm_constraints_output *output = crtc->constraints_output;

	if (!output)
		return;
	crtc->constraints_output = NULL;
	drm_constraints_catalog_close(output->catalog);
	drm_constraints_catalog_put(output->catalog);
	kfree(output);
}

struct drm_constraints_catalog *drm_constraints_crtc_catalog(struct drm_crtc *crtc)
{
	return crtc->constraints_output ? crtc->constraints_output->catalog : NULL;
}
EXPORT_SYMBOL_GPL(drm_constraints_crtc_catalog);

void drm_constraints_crtc_state_init(struct drm_crtc_state *state)
{
	if (state->crtc->constraints_output)
		state->constraints = drm_constraints_catalog_selected(state->crtc->constraints_output->catalog);
}
