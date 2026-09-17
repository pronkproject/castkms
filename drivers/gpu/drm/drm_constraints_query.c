// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/build_bug.h>
#include <linux/err.h>
#include <linux/export.h>
#include <linux/slab.h>
#include <linux/uaccess.h>
#include <drm/drm_constraints_encoding.h>
#include <drm/drm_constraints_entry.h>
#include <drm/drm_constraints_list.h>
#include <drm/drm_constraints_query.h>

static_assert(sizeof(struct drm_mode_list_constraints) == 48);
static_assert(offsetof(struct drm_mode_list_constraints, generation) == 8);
static_assert(offsetof(struct drm_mode_list_constraints, data) == 16);
static_assert(offsetof(struct drm_mode_list_constraints, size) == 24);
static_assert(offsetof(struct drm_mode_list_constraints, reserved) == 32);

int drm_constraints_list_copy_to_user(struct drm_constraints_list *list,
				      struct drm_mode_list_constraints *request)
{
	const struct drm_constraints_snapshot_info *info;
	const struct drm_constraints_listing *entries;
	struct drm_constraints_snapshot *snapshot;
	void *buffer = NULL;
	size_t required;
	int ret;

	if (!list || !request || !request->crtc_id || request->flags || request->pad ||
	    request->reserved[0] || request->reserved[1] || (!request->data != !request->size))
		return -EINVAL;
	snapshot = drm_constraints_list_snapshot(list, request->generation);
	if (IS_ERR(snapshot))
		return PTR_ERR(snapshot);
	info = drm_constraints_snapshot_info(snapshot);
	entries = drm_constraints_snapshot_entries(snapshot);
	if (drm_constraints_entry_crtc(entries[0].entry) != request->crtc_id) {
		ret = -EINVAL;
		goto out;
	}
	ret = drm_constraints_snapshot_encode(snapshot, NULL, 0, &required);
	if (ret)
		goto out;
	if (!request->data)
		goto metadata;
	if (request->size < required) {
		ret = -ENOSPC;
		goto metadata;
	}
	buffer = kvmalloc(required, GFP_KERNEL_ACCOUNT);
	if (!buffer) {
		ret = -ENOMEM;
		goto out;
	}
	ret = drm_constraints_snapshot_encode(snapshot, buffer, required, &required);
	if (ret)
		goto out;
	if (copy_to_user(u64_to_user_ptr(request->data), buffer, required)) {
		ret = -EFAULT;
		goto out;
	}
metadata:
	request->size = required;
	request->generation = info->generation;
out:
	kvfree(buffer);
	drm_constraints_snapshot_put(snapshot);
	return ret;
}
EXPORT_SYMBOL_GPL(drm_constraints_list_copy_to_user);
