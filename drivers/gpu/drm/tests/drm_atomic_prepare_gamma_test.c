// SPDX-License-Identifier: GPL-2.0 OR MIT

#include <linux/completion.h>
#include <linux/kthread.h>
#include <drm/drm_atomic.h>
#include <drm/drm_atomic_gamma.h>
#include <drm/drm_atomic_helper.h>
#include <drm/drm_atomic_prepare.h>
#include <drm/drm_atomic_prepare_display.h>
#include <drm/drm_atomic_prepare_owner.h>
#include <drm/drm_color_mgmt.h>
#include <drm/drm_kunit_helpers.h>
#include <drm/drm_property.h>
#include <kunit/test.h>

struct gamma_fixture {
	struct drm_device *dev;
	struct drm_crtc *crtc;
	struct drm_property_blob *table;
	struct drm_prepare_owner *owner;
	struct drm_prepare_read_claim *read;
	struct task_struct *worker;
	struct completion checked;
	unsigned int checks, installations;
	u16 before[6], during[6];
	int worker_error;
	bool revoke;
};

static int check_update(struct drm_device *dev, struct drm_atomic_commit *state)
{
	struct gamma_fixture *f = dev->dev_private;

	f->checks++;
	complete_all(&f->checked);
	return 0;
}

static int install_update(struct drm_device *dev, struct drm_atomic_commit *state, bool nonblock)
{
	struct gamma_fixture *f = dev->dev_private;
	int ret = drm_atomic_helper_swap_state(state, false);

	if (!ret)
		f->installations++;
	return ret;
}

static const struct drm_mode_config_funcs config_funcs = {
	.atomic_check = check_update,
	.atomic_commit = install_update,
};

static void finish_fixture(void *data)
{
	struct gamma_fixture *f = data;

	complete_all(&f->checked);
	if (f->worker)
		kthread_stop(f->worker);
	if (f->read)
		drm_prepare_read_abandon(f->read);
	drm_prepare_owner_put(f->owner);
	drm_property_blob_put(f->table);
}

static struct gamma_fixture *new_fixture(struct kunit *test)
{
	const struct drm_color_lut entries[] = {
		{ .red = 11, .green = 13, .blue = 17 },
		{ .red = 19, .green = 23, .blue = 29 },
	};
	struct gamma_fixture *f = kunit_kzalloc(test, sizeof(*f), GFP_KERNEL);
	struct device *parent = drm_kunit_helper_alloc_device(test);
	struct drm_plane *plane;
	struct drm_prepare_owner *owner;
	struct drm_property_blob *table;

	KUNIT_ASSERT_NOT_NULL(test, f);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, parent);
	f->dev = __drm_kunit_helper_alloc_drm_device(test, parent, sizeof(*f->dev), 0,
						  DRIVER_MODESET | DRIVER_ATOMIC);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->dev);
	f->dev->dev_private = f;
	f->dev->mode_config.funcs = &config_funcs;
	KUNIT_ASSERT_EQ(test, drm_atomic_prepare_display_init(f->dev, 8), 0);
	plane = drm_kunit_helper_create_primary_plane(test, f->dev, NULL, NULL, NULL, 0, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, plane);
	f->crtc = drm_kunit_helper_create_crtc(test, f->dev, plane, NULL, NULL, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, f->crtc);
	drm_crtc_enable_color_mgmt(f->crtc, 0, false, 2);
	KUNIT_ASSERT_EQ(test, drm_mode_crtc_set_gamma_size(f->crtc, 2), 0);
	drm_mode_config_reset(f->dev);
	memcpy(f->before, f->crtc->gamma_store, sizeof(f->before));
	init_completion(&f->checked);
	KUNIT_ASSERT_EQ(test, kunit_add_action_or_reset(test, finish_fixture, f), 0);
	owner = drm_prepare_owner_create(8);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, owner);
	f->owner = owner;
	table = drm_property_create_blob(f->dev, sizeof(entries), entries);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, table);
	f->table = table;
	return f;
}

static int finish_reader(void *data)
{
	struct gamma_fixture *f = data;
	int ret;

	wait_for_completion(&f->checked);
	ret = drm_modeset_lock(&f->crtc->mutex, NULL);
	if (!ret) {
		memcpy(f->during, f->crtc->gamma_store, sizeof(f->during));
		drm_modeset_unlock(&f->crtc->mutex);
	}
	f->worker_error = ret;
	if (f->revoke)
		drm_prepare_owner_revoke(f->owner);
	drm_prepare_read_release(f->read, NULL);
	f->read = NULL;
	set_current_state(TASK_INTERRUPTIBLE);
	while (!kthread_should_stop()) {
		schedule();
		set_current_state(TASK_INTERRUPTIBLE);
	}
	__set_current_state(TASK_RUNNING);
	return 0;
}

static void start_reader(struct kunit *test, struct gamma_fixture *f)
{
	struct drm_prepare_source *source;
	struct drm_prepare_read_claim *read = NULL;
	int ret = drm_modeset_lock(&f->crtc->mutex, NULL);

	KUNIT_ASSERT_EQ(test, ret, 0);
	source = drm_atomic_prepare_crtc_source(f->crtc);
	if (!IS_ERR(source))
		read = drm_prepare_source_claim(source);
	drm_modeset_unlock(&f->crtc->mutex);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, source);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, read);
	f->read = read;
	f->worker = kthread_run(finish_reader, f, "drm-gamma-reader");
	if (IS_ERR(f->worker)) {
		ret = PTR_ERR(f->worker);
		f->worker = NULL;
	}
	KUNIT_ASSERT_EQ(test, ret, 0);
}

static void join_reader(struct kunit *test, struct gamma_fixture *f)
{
	complete_all(&f->checked);
	kthread_stop(f->worker);
	f->worker = NULL;
	KUNIT_EXPECT_EQ(test, f->worker_error, 0);
}

static void gamma_command_waits_without_changing_readback(struct kunit *test)
{
	const u16 expected[] = { 11, 19, 13, 23, 17, 29 };
	struct gamma_fixture *f = new_fixture(test);
	int ret;

	start_reader(test, f);
	ret = drm_atomic_commit_legacy_gamma(f->crtc, f->table, f->owner, NULL, NULL);
	join_reader(test, f);
	KUNIT_EXPECT_EQ(test, ret, 0);
	KUNIT_EXPECT_GE(test, f->checks, 2);
	KUNIT_EXPECT_EQ(test, f->installations, 1);
	KUNIT_EXPECT_MEMEQ(test, f->before, f->during, sizeof(f->before));
	KUNIT_EXPECT_MEMEQ(test, expected, f->crtc->gamma_store, sizeof(expected));
}

static void revoked_gamma_command_preserves_readback(struct kunit *test)
{
	struct gamma_fixture *f = new_fixture(test);
	int ret;

	f->revoke = true;
	start_reader(test, f);
	ret = drm_atomic_commit_legacy_gamma(f->crtc, f->table, f->owner, NULL, NULL);
	join_reader(test, f);
	KUNIT_EXPECT_EQ(test, ret, -ECANCELED);
	KUNIT_EXPECT_EQ(test, f->installations, 0);
	KUNIT_EXPECT_MEMEQ(test, f->before, f->crtc->gamma_store, sizeof(f->before));
}

static struct kunit_case cases[] = {
	KUNIT_CASE(gamma_command_waits_without_changing_readback),
	KUNIT_CASE(revoked_gamma_command_preserves_readback),
	{}
};

static struct kunit_suite suite = {
	.name = "drm_atomic_prepare_gamma",
	.test_cases = cases,
};

kunit_test_suite(suite);
MODULE_LICENSE("GPL");
