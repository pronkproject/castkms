// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/slab.h>
#include <linux/uaccess.h>
#include <drm/drm_atomic_gamma.h>
#include <drm/drm_atomic_prepare_auth.h>
#include <drm/drm_atomic_prepare_owner.h>
#include <drm/drm_auth.h>
#include <drm/drm_color_mgmt.h>
#include <drm/drm_crtc.h>
#include <drm/drm_lease.h>
#include <drm/drm_property.h>

#include "drm_atomic_user_commit.h"

static int validate_gamma(struct drm_crtc *crtc, void *data)
{
	struct drm_file *file = data;

	if (!drm_is_current_master(file) || !drm_lease_held(file, crtc->base.id))
		return -EACCES;
	return 0;
}

static struct drm_property_blob *copy_gamma(struct drm_crtc *crtc,
					   const struct drm_mode_crtc_lut *input)
{
	struct drm_property_blob *table;
	struct drm_color_lut *entries;
	u16 *values, *green, *blue;
	u32 count = input->gamma_size, i;
	size_t bytes = array_size(count, sizeof(*values));

	values = kvmalloc_array(count, 3 * sizeof(*values), GFP_KERNEL);
	if (!values)
		return ERR_PTR(-ENOMEM);
	green = values + count;
	blue = green + count;
	if (copy_from_user(values, u64_to_user_ptr(input->red), bytes) ||
	    copy_from_user(green, u64_to_user_ptr(input->green), bytes) ||
	    copy_from_user(blue, u64_to_user_ptr(input->blue), bytes)) {
		table = ERR_PTR(-EFAULT);
		goto out;
	}
	table = drm_property_create_blob(crtc->dev, array_size(count, sizeof(*entries)), NULL);
	if (IS_ERR(table))
		goto out;
	entries = table->data;
	for (i = 0; i < count; i++) {
		entries[i].red = values[i];
		entries[i].green = green[i];
		entries[i].blue = blue[i];
	}
out:
	kvfree(values);
	return table;
}

int drm_atomic_commit_user_gamma(struct drm_crtc *crtc, const struct drm_mode_crtc_lut *input,
				  struct drm_file *file)
{
	struct drm_prepare_owner *owner;
	struct drm_property_blob *table;
	int ret;

	if (input->gamma_size != crtc->gamma_size || !input->gamma_size)
		return -EINVAL;
	if (crtc->funcs->gamma_set)
		return -EOPNOTSUPP;
	owner = drm_file_prepare_owner(file);
	if (IS_ERR(owner))
		return PTR_ERR(owner);
	table = copy_gamma(crtc, input);
	if (IS_ERR(table)) {
		ret = PTR_ERR(table);
		goto out;
	}
	ret = drm_atomic_commit_legacy_gamma(crtc, table, owner, validate_gamma, file);
	drm_property_blob_put(table);
out:
	drm_prepare_owner_put(owner);
	return ret;
}
