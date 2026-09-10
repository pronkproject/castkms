// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/err.h>
#include <linux/export.h>
#include <drm/drm_atomic.h>
#include <drm/drm_atomic_prepare_commit.h>
#include <drm/drm_atomic_prepare_display.h>
#include <drm/drm_atomic_prepare_outputs.h>
#include <drm/drm_atomic_prepare_request.h>
#include <drm/drm_atomic_prepare_ticket.h>
#include <drm/drm_device.h>
#include <drm/drm_drv.h>

static int prepare_request(struct drm_atomic_commit *state,
			   struct drm_prepare_owner *owner,
			   struct drm_prepare_ticket **ticket)
{
	struct drm_prepare_output_generation entries[DRM_PREPARE_MAX_OUTPUTS];
	struct drm_prepare_ticket *next;
	int count;

	count = drm_atomic_prepare_display_observe(state, entries, ARRAY_SIZE(entries));
	if (count < 0)
		return count;
	next = owner ? drm_prepare_ticket_create_owned(owner, entries, count) :
		       drm_prepare_ticket_create(entries, count);
	if (IS_ERR(next))
		return PTR_ERR(next);

	/* Acquire replacement holds before releasing those retained across a wait. */
	if (*ticket)
		drm_prepare_ticket_put(*ticket);
	*ticket = next;
	if (owner)
		return drm_atomic_commit_prepare_owned(state, next, owner,
						      drm_atomic_prepare_display_observe);
	return drm_atomic_commit_prepare(state, next, drm_atomic_prepare_display_observe);
}

static int commit_request(struct drm_device *dev, struct drm_prepare_owner *owner,
			  int (*build)(struct drm_atomic_commit *state, void *data),
			  void *data)
{
	struct drm_prepare_ticket *ticket = NULL;
	struct drm_modeset_acquire_ctx ctx;
	struct drm_atomic_commit *state = NULL;
	int ret;

	if (!build || !drm_core_check_feature(dev, DRIVER_ATOMIC))
		return -EINVAL;
	drm_modeset_acquire_init(&ctx, owner ? DRM_MODESET_ACQUIRE_INTERRUPTIBLE : 0);

	for (;;) {
		state = drm_atomic_commit_alloc(dev);
		if (!state) {
			ret = -ENOMEM;
			break;
		}
		state->acquire_ctx = &ctx;
		ret = build(state, data);
		if (ret == DRM_ATOMIC_REQUEST_UNCHANGED) {
			ret = 0;
			break;
		}
		if (ret > 0) {
			ret = -EINVAL;
			break;
		}
		if (!ret)
			ret = drm_atomic_check_only(state);
		if (ret)
			goto retry_lock;

		if (dev->mode_config.preparation) {
			ret = prepare_request(state, owner, &ticket);
			if (ret == -EAGAIN) {
				drm_atomic_commit_put(state);
				state = NULL;
				drm_modeset_drop_locks(&ctx);
				ret = drm_prepare_ticket_wait(ticket);
				if (!ret)
					continue;
			}
			if (ret)
				break;
		}

		/* Checking and ticket capture used the same still-locked state. */
		ret = dev->mode_config.funcs->atomic_commit(dev, state, false);
retry_lock:
		if (ret != -EDEADLK)
			break;
		drm_atomic_commit_put(state);
		state = NULL;
		ret = drm_modeset_backoff(&ctx);
		if (ret)
			break;
	}

	if (state)
		drm_atomic_commit_put(state);
	drm_modeset_drop_locks(&ctx);
	drm_modeset_acquire_fini(&ctx);
	if (ticket)
		drm_prepare_ticket_put(ticket);
	return ret;
}

int drm_atomic_commit_request(struct drm_device *dev,
			      int (*build)(struct drm_atomic_commit *state, void *data),
			      void *data)
{
	return commit_request(dev, NULL, build, data);
}
EXPORT_SYMBOL_GPL(drm_atomic_commit_request);

int drm_atomic_commit_request_owned(struct drm_device *dev,
				    struct drm_prepare_owner *owner,
				    int (*build)(struct drm_atomic_commit *state, void *data),
				    void *data)
{
	if (!owner)
		return -EINVAL;
	if (!dev->mode_config.preparation)
		return -EOPNOTSUPP;
	return commit_request(dev, owner, build, data);
}
EXPORT_SYMBOL_GPL(drm_atomic_commit_request_owned);
