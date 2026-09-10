// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/err.h>
#include <linux/export.h>
#include <linux/slab.h>
#include <drm/drm_atomic.h>
#include <drm/drm_atomic_prepare.h>
#include <drm/drm_atomic_prepare_display.h>
#include <drm/drm_atomic_prepare_outputs.h>
#include <drm/drm_atomic_prepare_ticket.h>
#include <drm/drm_device.h>
#include <drm/drm_drv.h>
#include <drm/drm_property.h>

struct drm_prepare_display {
	struct drm_prepare_domain *domain;
	unsigned int capacity;
};

int drm_atomic_prepare_display_init(struct drm_device *dev, unsigned int capacity)
{
	struct drm_prepare_display *display;

	if (!capacity || !drm_core_check_feature(dev, DRIVER_ATOMIC))
		return -EINVAL;
	if (dev->mode_config.preparation || dev->mode_config.num_crtc)
		return -EBUSY;
	display = kzalloc_obj(*display);
	if (!display)
		return -ENOMEM;
	display->domain = drm_prepare_domain_create();
	if (IS_ERR(display->domain)) {
		int ret = PTR_ERR(display->domain);

		kfree(display);
		return ret;
	}
	display->capacity = capacity;
	dev->mode_config.prop_prepare_fd = drm_property_create_signed_range(dev,
					DRM_MODE_PROP_ATOMIC, "PREPARE_FD", -1, INT_MAX);
	if (!dev->mode_config.prop_prepare_fd) {
		drm_prepare_domain_put(display->domain);
		kfree(display);
		return -ENOMEM;
	}
	dev->mode_config.preparation = display;
	return 0;
}
EXPORT_SYMBOL_GPL(drm_atomic_prepare_display_init);

void drm_atomic_prepare_display_fini(struct drm_device *dev)
{
	struct drm_prepare_display *display = dev->mode_config.preparation;

	if (!display)
		return;
	dev->mode_config.preparation = NULL;
	drm_prepare_domain_put(display->domain);
	kfree(display);
}

static int ensure_source(struct drm_crtc_state *state)
{
	struct drm_prepare_display *display = state->crtc->dev->mode_config.preparation;
	struct drm_prepare_source *source;

	if (!display)
		return -EOPNOTSUPP;
	if (state->prepare_source)
		return 0;
	source = drm_prepare_source_create_in(display->domain, display->capacity);
	if (IS_ERR(source))
		return PTR_ERR(source);
	state->prepare_source = source;
	return 0;
}

struct drm_prepare_source *drm_atomic_prepare_crtc_source(struct drm_crtc *crtc)
{
	int ret;

	drm_modeset_lock_assert_held(&crtc->mutex);
	if (!crtc->state)
		return ERR_PTR(-EINVAL);
	ret = ensure_source(crtc->state);
	return ret ? ERR_PTR(ret) : crtc->state->prepare_source;
}
EXPORT_SYMBOL_GPL(drm_atomic_prepare_crtc_source);

struct drm_prepare_ticket *
drm_atomic_prepare_crtcs(struct drm_crtc * const *crtcs, unsigned int count,
			 struct drm_prepare_owner *owner)
{
	struct drm_prepare_output_generation entries[DRM_PREPARE_MAX_OUTPUTS];
	unsigned int i;

	if (!count || !crtcs)
		return ERR_PTR(-EINVAL);
	if (count > ARRAY_SIZE(entries))
		return ERR_PTR(-E2BIG);
	for (i = 0; i < count; i++) {
		struct drm_prepare_source *source;

		if (!crtcs[i])
			return ERR_PTR(-EINVAL);
		if (crtcs[i]->dev != crtcs[0]->dev)
			return ERR_PTR(-EXDEV);
		source = drm_atomic_prepare_crtc_source(crtcs[i]);
		if (IS_ERR(source))
			return ERR_CAST(source);
		entries[i] = (struct drm_prepare_output_generation) {
			.crtc_id = crtcs[i]->base.id,
			.source = source,
		};
	}
	return owner ? drm_prepare_ticket_create_owned(owner, entries, count) :
		       drm_prepare_ticket_create(entries, count);
}
EXPORT_SYMBOL_GPL(drm_atomic_prepare_crtcs);

int drm_atomic_prepare_display_check(struct drm_atomic_commit *state)
{
	struct drm_crtc *crtc;
	struct drm_crtc_state *old, *new;
	int i, ret;

	if (!state->dev->mode_config.preparation)
		return 0;
	if (state->async_update)
		return -EOPNOTSUPP;
	for_each_oldnew_crtc_in_state(state, crtc, old, new, i) {
		drm_modeset_lock_assert_held(&crtc->mutex);
		ret = ensure_source(old);
		if (ret)
			return ret;
		ret = ensure_source(new);
		if (ret)
			return ret;
	}
	return 0;
}

int drm_atomic_prepare_display_observe(struct drm_atomic_commit *state,
				       struct drm_prepare_output_generation *entries,
				       unsigned int capacity)
{
	struct drm_crtc *crtc;
	struct drm_crtc_state *old;
	unsigned int count = 0;
	int i;

	if (!state->dev->mode_config.preparation)
		return -EOPNOTSUPP;
	for_each_old_crtc_in_state(state, crtc, old, i) {
		drm_modeset_lock_assert_held(&crtc->mutex);
		if (count == capacity)
			return -E2BIG;
		if (!old->prepare_source)
			return -EINVAL;
		entries[count++] = (struct drm_prepare_output_generation) {
			.crtc_id = crtc->base.id,
			.source = old->prepare_source,
		};
	}
	return count;
}
EXPORT_SYMBOL_GPL(drm_atomic_prepare_display_observe);
