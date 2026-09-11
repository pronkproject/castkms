// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/overflow.h>
#include <linux/slab.h>
#include <linux/string.h>
#include <linux/uaccess.h>
#include <drm/drm_mode.h>

#include "drm_atomic_user_input.h"

void drm_atomic_free_user_input(struct drm_atomic_user_input *input)
{
	if (!input)
		return;
	kvfree(input->values);
	kvfree(input->properties);
	kvfree(input->counts);
	kvfree(input->objects);
	kfree(input);
}

/* Each array is read once; userspace must keep the inputs stable during copying. */
struct drm_atomic_user_input *drm_atomic_copy_user_input(const struct drm_mode_atomic *arg)
{
	struct drm_atomic_user_input *input;
	void *copy;
	u32 i;
	int ret;

	input = kzalloc_obj(*input);
	if (!input)
		return ERR_PTR(-ENOMEM);
	input->object_count = arg->count_objs;
	if (!input->object_count)
		return input;
	copy = vmemdup_array_user(u64_to_user_ptr(arg->objs_ptr), input->object_count, sizeof(u32));
	if (IS_ERR(copy))
		goto copy_failed;
	input->objects = copy;
	copy = vmemdup_array_user(u64_to_user_ptr(arg->count_props_ptr),
				 input->object_count, sizeof(u32));
	if (IS_ERR(copy))
		goto copy_failed;
	input->counts = copy;
	for (i = 0; i < input->object_count; i++) {
		if (check_add_overflow(input->property_count, input->counts[i],
				       &input->property_count)) {
			ret = -EOVERFLOW;
			goto fail;
		}
	}
	if (!input->property_count)
		return input;
	copy = vmemdup_array_user(u64_to_user_ptr(arg->props_ptr), input->property_count, sizeof(u32));
	if (IS_ERR(copy))
		goto copy_failed;
	input->properties = copy;
	copy = vmemdup_array_user(u64_to_user_ptr(arg->prop_values_ptr),
				 input->property_count, sizeof(u64));
	if (IS_ERR(copy))
		goto copy_failed;
	input->values = copy;
	return input;

copy_failed:
	ret = PTR_ERR(copy);
fail:
	drm_atomic_free_user_input(input);
	return ERR_PTR(ret);
}
