// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/module.h>
#include <drm/drm_atomic_state_helper.h>
#include <drm/drm_blend.h>
#include <drm/drm_constraints.h>
#include <drm/drm_constraints_list.h>
#include <drm/drm_constraints_device.h>
#include <drm/drm_constraints_entry.h>
#include <drm/drm_constraints_output.h>
#include <drm/drm_crtc.h>
#include <drm/drm_fourcc.h>
#include <drm/drm_kunit_helpers.h>
#include <drm/drm_property.h>
#include <kunit/device.h>
#include <kunit/test.h>

struct output_fixture {
	struct drm_device drm;
	struct drm_plane *plane;
	struct drm_crtc *crtc;
	unsigned int released;
};

static void release_backend(void *data)
{
	struct output_fixture *fixture = data;

	fixture->released++;
}

static const struct drm_constraints_entry_ops entry_ops = {
	.owner = THIS_MODULE,
	.release = release_backend,
};

static int check(const struct drm_atomic_commit *state,
		 const struct drm_crtc_state *crtc, void *backend)
{
	return 0;
}

static const struct drm_constraints_output_ops output_ops = { .check = check };

static void put_description(void *data) { drm_constraints_description_put(data); }
static void put_entry(void *data) { drm_constraints_entry_put(data); }
static void put_state(void *data) { drm_atomic_helper_crtc_destroy_state(NULL, data); }

static struct output_fixture *new_fixture(struct kunit *test, const char *name)
{
	struct output_fixture *fixture;
	struct device *dev;

	dev = kunit_device_register(test, name);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, dev);
	fixture = drm_kunit_helper_alloc_drm_device(test, dev, struct output_fixture, drm,
						   DRIVER_MODESET | DRIVER_ATOMIC);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, fixture);
	KUNIT_ASSERT_EQ(test, drm_constraints_device_init(&fixture->drm, 8), 0);
	fixture->plane = drm_kunit_helper_create_primary_plane(test, &fixture->drm,
							     NULL, NULL, NULL, 0, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, fixture->plane);
	fixture->crtc = drm_kunit_helper_create_crtc(test, &fixture->drm, fixture->plane,
						    NULL, NULL, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, fixture->crtc);
	drm_mode_config_reset(&fixture->drm);
	return fixture;
}

static struct drm_constraints_entry *
new_entry(struct kunit *test, struct output_fixture *fixture, u32 crtc_id, u32 plane_id)
{
	const struct drm_constraints_size size = { 64, 32, 64, 32 };
	const struct drm_constraints_format format = {
		.plane_id = plane_id,
		.format = DRM_FORMAT_XRGB8888,
		.modifier = DRM_FORMAT_MOD_LINEAR,
		.size = size,
	};
	struct drm_constraints_description *description;
	struct drm_constraints_entry *entry;

	description = drm_constraints_description_create(&size, &format, 1, NULL, 0);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, description);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_description, description), 0);
	entry = drm_constraints_entry_create(drm_constraints_device_domain(&fixture->drm),
					     crtc_id, description, &entry_ops, fixture);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, entry);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_entry, entry), 0);
	return entry;
}

static void reset_and_pristine_state_retain_accepted_binding(struct kunit *test)
{
	struct output_fixture *fixture = new_fixture(test, "constraints-output");
	struct drm_constraints_entry *entry = new_entry(test, fixture, fixture->crtc->base.id,
							fixture->plane->base.id);
	struct drm_crtc_state *pristine;

	KUNIT_ASSERT_EQ(test, drm_constraints_crtc_init(fixture->crtc, entry, 4, &output_ops), 0);
	KUNIT_EXPECT_PTR_EQ(test, fixture->crtc->state->constraints, entry);
	pristine = drm_atomic_helper_crtc_create_state(fixture->crtc);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, pristine);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_state, pristine), 0);
	KUNIT_EXPECT_PTR_EQ(test, pristine->constraints, entry);
	drm_constraints_list_close(drm_constraints_crtc_list(fixture->crtc));
	drm_mode_config_reset(&fixture->drm);
	KUNIT_EXPECT_PTR_EQ(test, fixture->crtc->state->constraints, entry);
	KUNIT_EXPECT_EQ(test, fixture->released, 0);
}

static int accept_entry(struct drm_constraints_entry *entry, void *data)
{
	return 0;
}

static void fixed_default_survives_withdrawal_and_selection(struct kunit *test)
{
	struct output_fixture *f = new_fixture(test, "constraints-default");
	struct drm_constraints_entry *initial = new_entry(test, f, f->crtc->base.id,
							  f->plane->base.id);
	struct drm_constraints_entry *target = new_entry(test, f, f->crtc->base.id,
							 f->plane->base.id);
	struct drm_constraints_list *list;
	int ret;

	KUNIT_EXPECT_PTR_EQ(test, drm_constraints_crtc_default(f->crtc), NULL);
	KUNIT_ASSERT_EQ(test, drm_constraints_crtc_init(f->crtc, initial, 4, &output_ops), 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_crtc_add(f->crtc, target), 0);
	list = drm_constraints_crtc_list(f->crtc);
	KUNIT_ASSERT_EQ(test, drm_modeset_lock(&f->crtc->mutex, NULL), 0);
	ret = drm_constraints_list_accept(list, target, accept_entry, NULL);
	drm_modeset_unlock(&f->crtc->mutex);
	KUNIT_ASSERT_EQ(test, ret, 0);
	drm_mode_config_reset(&f->drm);
	KUNIT_EXPECT_PTR_EQ(test, f->crtc->state->constraints, target);
	KUNIT_ASSERT_EQ(test, drm_constraints_list_withdraw(list,
							    drm_constraints_entry_id(initial)), 0);
	KUNIT_ASSERT_EQ(test, drm_constraints_list_forget(list,
							  drm_constraints_entry_id(initial)), 0);
	kunit_release_action(test, put_entry, initial);
	KUNIT_EXPECT_EQ(test, f->released, 0);
	KUNIT_EXPECT_PTR_EQ(test, drm_constraints_crtc_default(f->crtc), initial);
	drm_constraints_list_close(list);
	KUNIT_EXPECT_PTR_EQ(test, drm_constraints_crtc_default(f->crtc), initial);
}

static void attaching_rejects_foreign_device_and_objects(struct kunit *test)
{
	struct output_fixture *fixture = new_fixture(test, "constraints-output");
	struct output_fixture *other = new_fixture(test, "constraints-foreign");
	struct drm_constraints_entry *foreign = new_entry(test, other, fixture->crtc->base.id,
							  fixture->plane->base.id);
	struct drm_constraints_entry *wrong_crtc = new_entry(test, fixture, fixture->crtc->base.id + 1,
							     fixture->plane->base.id);
	struct drm_constraints_entry *wrong_plane = new_entry(test, fixture, fixture->crtc->base.id,
							      U32_MAX);

	KUNIT_EXPECT_EQ(test, drm_constraints_crtc_init(fixture->crtc, foreign, 4, &output_ops), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_constraints_crtc_init(fixture->crtc, wrong_crtc, 4, &output_ops), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_constraints_crtc_init(fixture->crtc, wrong_plane, 4, &output_ops), -EINVAL);
	KUNIT_EXPECT_PTR_EQ(test, drm_constraints_crtc_list(fixture->crtc), NULL);
	KUNIT_EXPECT_PTR_EQ(test, fixture->crtc->state->constraints, NULL);
}

static void attaching_requires_disabled_unregistered_output(struct kunit *test)
{
	struct output_fixture *fixture = new_fixture(test, "constraints-output");
	struct drm_constraints_entry *entry = new_entry(test, fixture, fixture->crtc->base.id,
							fixture->plane->base.id);

	fixture->drm.registered = true;
	KUNIT_EXPECT_EQ(test, drm_constraints_crtc_init(fixture->crtc, entry, 4, &output_ops), -EBUSY);
	fixture->drm.registered = false;
	fixture->crtc->state->enable = true;
	KUNIT_EXPECT_EQ(test, drm_constraints_crtc_init(fixture->crtc, entry, 4, &output_ops), -EBUSY);
	fixture->crtc->state->enable = false;
	KUNIT_ASSERT_EQ(test, drm_constraints_crtc_init(fixture->crtc, entry, 4, &output_ops), 0);
	KUNIT_EXPECT_EQ(test, drm_constraints_crtc_init(fixture->crtc, entry, 4, &output_ops), -EBUSY);
	KUNIT_EXPECT_EQ(test, drm_constraints_device_init(&fixture->drm, 8), -EBUSY);
}

static void publishing_revalidates_complete_object_scope(struct kunit *test)
{
	struct output_fixture *fixture = new_fixture(test, "constraints-output");
	struct output_fixture *other = new_fixture(test, "constraints-foreign");
	struct drm_constraints_entry *initial = new_entry(test, fixture, fixture->crtc->base.id,
							  fixture->plane->base.id);
	struct drm_constraints_entry *target = new_entry(test, fixture, fixture->crtc->base.id,
							 fixture->plane->base.id);
	struct drm_constraints_entry *foreign = new_entry(test, other, fixture->crtc->base.id,
							  fixture->plane->base.id);
	struct drm_constraints_entry *wrong_plane = new_entry(test, fixture, fixture->crtc->base.id,
							      U32_MAX);
	struct drm_constraints_entry *selected;

	KUNIT_EXPECT_EQ(test, drm_constraints_crtc_add(fixture->crtc, target), -EOPNOTSUPP);
	KUNIT_ASSERT_EQ(test, drm_constraints_crtc_init(fixture->crtc, initial, 4, &output_ops), 0);
	KUNIT_EXPECT_EQ(test, drm_constraints_crtc_add(fixture->crtc, foreign), -EINVAL);
	KUNIT_EXPECT_EQ(test, drm_constraints_crtc_add(fixture->crtc, wrong_plane), -EINVAL);
	KUNIT_ASSERT_EQ(test, drm_constraints_crtc_add(fixture->crtc, target), 0);
	selected = drm_constraints_list_selected(drm_constraints_crtc_list(fixture->crtc));
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_entry, selected), 0);
	KUNIT_EXPECT_PTR_EQ(test, selected, initial);
}

static void offers_require_advertised_plane_allocations(struct kunit *test)
{
	struct output_fixture *fixture = new_fixture(test, "constraints-output");
	const struct drm_constraints_size size = { 64, 32, 64, 32 };
	struct drm_constraints_format format = {
		.plane_id = fixture->plane->base.id,
		.format = DRM_FORMAT_XRGB8888,
		.modifier = I915_FORMAT_MOD_X_TILED,
		.size = size,
	};
	struct drm_constraints_entry *initial = new_entry(test, fixture,
					fixture->crtc->base.id, fixture->plane->base.id);
	struct drm_constraints_description *description;
	struct drm_constraints_entry *entry;
	unsigned int i;

	KUNIT_ASSERT_EQ(test, drm_constraints_crtc_init(fixture->crtc, initial, 4, &output_ops), 0);
	for (i = 0; i < 2; i++) {
		description = drm_constraints_description_create(&size, &format, 1, NULL, 0);
		KUNIT_ASSERT_NOT_ERR_OR_NULL(test, description);
		KUNIT_ASSERT_EQ(test,
				kunit_add_action_or_reset(test, put_description, description), 0);
		entry = drm_constraints_entry_create(drm_constraints_device_domain(&fixture->drm),
				fixture->crtc->base.id, description, &entry_ops, fixture);
		KUNIT_ASSERT_NOT_ERR_OR_NULL(test, entry);
		KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_entry, entry), 0);
		KUNIT_EXPECT_EQ(test, drm_constraints_crtc_add(fixture->crtc, entry), -EINVAL);
		format.format = DRM_FORMAT_NV12;
		format.modifier = DRM_FORMAT_MOD_LINEAR;
	}
}

static struct drm_constraints_entry *
new_property_entry(struct kunit *test, struct output_fixture *fixture,
		   const struct drm_constraints_property *rule)
{
	const struct drm_constraints_size size = { 64, 32, 64, 32 };
	const struct drm_constraints_format format = {
		.plane_id = fixture->plane->base.id, .format = DRM_FORMAT_XRGB8888,
		.modifier = DRM_FORMAT_MOD_LINEAR, .size = size,
	};
	struct drm_constraints_description *description;
	struct drm_constraints_entry *entry;

	description = drm_constraints_description_create(&size, &format, 1, rule, 1);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, description);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_description, description), 0);
	entry = drm_constraints_entry_create(drm_constraints_device_domain(&fixture->drm),
					     fixture->crtc->base.id, description,
					     &entry_ops, fixture);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, entry);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, put_entry, entry), 0);
	return entry;
}

static void property_rules_require_attached_supported_domains(struct kunit *test)
{
	struct output_fixture *fixture = new_fixture(test, "constraints-properties");
	struct drm_constraints_property rule;
	struct drm_constraints_entry *entry;
	struct drm_property *immutable;
	unsigned int i;

	KUNIT_ASSERT_EQ(test, drm_plane_create_alpha_property(fixture->plane), 0);
	KUNIT_ASSERT_EQ(test, drm_plane_create_rotation_property(fixture->plane,
			DRM_MODE_ROTATE_0, DRM_MODE_ROTATE_0 | DRM_MODE_ROTATE_90), 0);
	immutable = drm_property_create_range(&fixture->drm, DRM_MODE_PROP_IMMUTABLE,
					      "fixed scalar", 0, 10);
	KUNIT_ASSERT_NOT_NULL(test, immutable);
	drm_object_attach_property(&fixture->plane->base, immutable, 0);
	rule = (struct drm_constraints_property) {
		.object_id = fixture->plane->base.id,
		.property_id = fixture->plane->alpha_property->base.id,
		.type = DRM_MODE_PROP_RANGE, .minimum = 1, .maximum = 32768,
	};
	entry = new_property_entry(test, fixture, &rule);
	KUNIT_ASSERT_EQ(test, drm_constraints_crtc_init(fixture->crtc, entry, 4, &output_ops), 0);
	for (i = 0; i < 7; i++) {
		struct drm_constraints_property invalid = rule;
		int expected = -EINVAL;

		switch (i) {
		case 0:
			invalid.object_id = fixture->crtc->base.id;
			break;
		case 1:
			invalid.property_id = U32_MAX;
			break;
		case 2:
			invalid.type = DRM_MODE_PROP_ENUM;
			invalid.minimum = invalid.maximum = 0;
			invalid.mask = 1;
			break;
		case 3:
			invalid.maximum = 65536;
			break;
		case 4:
			invalid.property_id = immutable->base.id;
			invalid.maximum = 10;
			break;
		case 5:
			invalid.property_id = fixture->plane->rotation_property->base.id;
			invalid.type = DRM_MODE_PROP_BITMASK;
			invalid.minimum = invalid.maximum = 0;
			invalid.mask = DRM_MODE_REFLECT_Y;
			break;
		case 6:
			invalid.property_id = fixture->drm.mode_config.prop_in_fence_fd->base.id;
			invalid.type = DRM_MODE_PROP_SIGNED_RANGE;
			invalid.minimum = invalid.maximum = U64_MAX;
			expected = -EOPNOTSUPP;
			break;
		}
		entry = new_property_entry(test, fixture, &invalid);
		KUNIT_EXPECT_EQ(test, drm_constraints_crtc_add(fixture->crtc, entry), expected);
		kunit_release_action(test, put_entry, entry);
	}
}

static struct kunit_case drm_constraints_output_tests[] = {
	KUNIT_CASE(reset_and_pristine_state_retain_accepted_binding),
	KUNIT_CASE(fixed_default_survives_withdrawal_and_selection),
	KUNIT_CASE(attaching_rejects_foreign_device_and_objects),
	KUNIT_CASE(attaching_requires_disabled_unregistered_output),
	KUNIT_CASE(publishing_revalidates_complete_object_scope),
	KUNIT_CASE(offers_require_advertised_plane_allocations),
	KUNIT_CASE(property_rules_require_attached_supported_domains),
	{}
};

static struct kunit_suite drm_constraints_output_test_suite = {
	.name = "drm_constraints_output",
	.test_cases = drm_constraints_output_tests,
};

kunit_test_suite(drm_constraints_output_test_suite);

MODULE_DESCRIPTION("DRM constraints output scope tests");
MODULE_LICENSE("Dual MIT/GPL");
