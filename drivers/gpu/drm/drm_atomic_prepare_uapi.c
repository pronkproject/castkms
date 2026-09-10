// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/file.h>
#include <linux/uaccess.h>
#include <drm/drm_atomic.h>
#include <drm/drm_atomic_prepare_auth.h>
#include <drm/drm_atomic_prepare_display.h>
#include <drm/drm_atomic_prepare_file.h>
#include <drm/drm_atomic_prepare_outputs.h>
#include <drm/drm_atomic_prepare_owner.h>
#include <drm/drm_atomic_prepare_submission.h>
#include <drm/drm_atomic_prepare_ticket.h>
#include <drm/drm_crtc.h>
#include <drm/drm_device.h>
#include <drm/drm_file.h>
#include <uapi/drm/drm_prepare.h>

#include "drm_atomic_prepare_uapi.h"

int drm_atomic_prepare_set_fd(struct drm_atomic_commit *state,
			     struct drm_crtc *crtc, u64 value)
{
	struct drm_prepare_ticket *ticket;
	struct file *file;
	int ret;

	if (value == U64_MAX)
		return drm_atomic_prepare_submission_set(state, crtc, NULL);
	if (value > INT_MAX)
		return -EINVAL;
	file = fget(value);
	if (!file)
		return -EBADF;
	ticket = drm_prepare_ticket_file_get_ticket(file);
	fput(file);
	if (IS_ERR(ticket))
		return PTR_ERR(ticket);
	ret = drm_atomic_prepare_submission_set(state, crtc, ticket);
	drm_prepare_ticket_put(ticket);
	return ret;
}

int drm_mode_prepare_replace_ioctl(struct drm_device *dev, void *data,
				   struct drm_file *file)
{
	struct drm_mode_prepare_replace *arg = data;
	struct drm_crtc *crtcs[DRM_PREPARE_MAX_OUTPUTS];
	u32 ids[DRM_PREPARE_MAX_OUTPUTS];
	struct drm_modeset_acquire_ctx ctx;
	struct drm_prepare_owner *owner;
	struct drm_prepare_ticket *ticket;
	struct file *ticket_file;
	unsigned int i, j;
	int ret, fd;

	if (!file->atomic || !file->atomic_preparation || !dev->mode_config.preparation)
		return -EOPNOTSUPP;
	if (arg->flags || arg->reserved[0] || arg->reserved[1] || !arg->count_crtcs)
		return -EINVAL;
	if (arg->count_crtcs > ARRAY_SIZE(ids))
		return -E2BIG;
	if (copy_from_user(ids, u64_to_user_ptr(arg->crtc_ids),
			   array_size(arg->count_crtcs, sizeof(*ids))))
		return -EFAULT;
	for (i = 0; i < arg->count_crtcs; i++) {
		if (!ids[i])
			return -EINVAL;
		for (j = 0; j < i; j++) {
			if (ids[i] == ids[j])
				return -EINVAL;
		}
	}
	owner = drm_file_prepare_owner(file);
	if (IS_ERR(owner))
		return PTR_ERR(owner);
	fd = get_unused_fd_flags(O_CLOEXEC);
	if (fd < 0) {
		ret = fd;
		goto put_owner;
	}
	drm_modeset_acquire_init(&ctx, DRM_MODESET_ACQUIRE_INTERRUPTIBLE);
retry:
	ret = drm_modeset_lock_all_ctx(dev, &ctx);
	if (ret)
		goto unlock;
	for (i = 0; i < arg->count_crtcs; i++) {
		crtcs[i] = drm_crtc_find(dev, file, ids[i]);
		if (!crtcs[i]) {
			ret = -ENOENT;
			goto unlock;
		}
	}
	ticket = drm_atomic_prepare_crtcs(crtcs, arg->count_crtcs, owner);
	if (IS_ERR(ticket)) {
		ret = PTR_ERR(ticket);
		goto unlock;
	}
	ticket_file = drm_prepare_ticket_file_create(ticket);
	drm_prepare_ticket_put(ticket);
	ret = IS_ERR(ticket_file) ? PTR_ERR(ticket_file) : 0;
unlock:
	if (ret == -EDEADLK) {
		ret = drm_modeset_backoff(&ctx);
		if (!ret)
			goto retry;
	}
	drm_modeset_drop_locks(&ctx);
	drm_modeset_acquire_fini(&ctx);
	if (ret)
		put_unused_fd(fd);
	else {
		fd_install(fd, ticket_file);
		ret = fd;
	}
put_owner:
	drm_prepare_owner_put(owner);
	return ret;
}
