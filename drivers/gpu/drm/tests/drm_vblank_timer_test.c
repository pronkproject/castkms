// SPDX-License-Identifier: GPL-2.0

#include <drm/drm_crtc.h>
#include <drm/drm_drv.h>
#include <drm/drm_kunit_helpers.h>
#include <drm/drm_modes.h>
#include <drm/drm_modeset_helper_vtables.h>
#include <drm/drm_vblank.h>

#include <kunit/test.h>
#include <linux/completion.h>
#include <linux/cpu.h>
#include <linux/hrtimer.h>
#include <linux/smp.h>
#include <linux/workqueue.h>

struct timer_test_device {
	struct drm_device drm;
	struct completion entered;
	atomic_t release;
	bool timed_out;
};

static bool gated_vblank(struct drm_crtc *crtc)
{
	struct timer_test_device *ctx = container_of(crtc->dev, typeof(*ctx), drm);
	ktime_t deadline = ktime_add_ms(ktime_get(), 1000);

	complete(&ctx->entered);
	/* Bound the gate so a failing restart test still releases its timer. */
	while (!atomic_read(&ctx->release)) {
		if (ktime_after(ktime_get(), deadline)) {
			WRITE_ONCE(ctx->timed_out, true);
			break;
		}
		cpu_relax();
	}
	return false;
}

static const struct drm_crtc_helper_funcs timer_helpers = {
	.handle_vblank_timeout = gated_vblank,
};

struct start_timer {
	struct drm_crtc *crtc;
	int result;
};

static long start_on_cpu(void *data)
{
	struct start_timer *start = data;
	struct drm_device *drm = start->crtc->dev;
	unsigned long flags;

	spin_lock_irqsave(&drm->vbl_lock, flags);
	spin_lock(&drm->vblank_time_lock);
	start->result = drm_crtc_vblank_start_timer(start->crtc);
	if (!start->result) {
		/* Keep the gated callback off the test thread's CPU. */
		hrtimer_start(&drm_crtc_vblank_crtc(start->crtc)->vblank_timer.timer,
			      ms_to_ktime(10), HRTIMER_MODE_REL_PINNED);
	}
	spin_unlock(&drm->vblank_time_lock);
	spin_unlock_irqrestore(&drm->vbl_lock, flags);
	return 0;
}

static void drm_vblank_restart_does_not_wait_for_callback(struct kunit *test)
{
	struct timer_test_device *ctx;
	struct drm_plane *plane;
	struct drm_crtc *crtc;
	struct device *dev;
	struct start_timer start;
	unsigned long flags, entered;
	int cpu, ret;

	dev = drm_kunit_helper_alloc_device(test);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, dev);
	ctx = drm_kunit_helper_alloc_drm_device(test, dev, struct timer_test_device,
					       drm, DRIVER_MODESET);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, ctx);
	init_completion(&ctx->entered);
	atomic_set(&ctx->release, 0);
	plane = drm_kunit_helper_create_primary_plane(test, &ctx->drm, NULL, NULL,
						     NULL, 0, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, plane);
	crtc = drm_kunit_helper_create_crtc(test, &ctx->drm, plane, NULL, NULL,
					  &timer_helpers);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, crtc);
	KUNIT_ASSERT_EQ(test, drm_vblank_init(&ctx->drm, 1), 0);
	crtc->mode = (struct drm_display_mode) {
		DRM_MODE("1024x768", 0, 65000, 1024, 1048, 1184, 1344, 0,
			 768, 771, 777, 806, 0, DRM_MODE_FLAG_NHSYNC | DRM_MODE_FLAG_NVSYNC)
	};
	drm_mode_set_crtcinfo(&crtc->mode, 0);
	start.crtc = crtc;
	start.result = -EINVAL;

	cpus_read_lock();
	migrate_disable();
	cpu = cpumask_any_but(cpu_online_mask, smp_processor_id());
	if (cpu >= nr_cpu_ids) {
		migrate_enable();
		cpus_read_unlock();
		kunit_skip(test, "Requires a second CPU for the running timer");
		return;
	}
	ret = work_on_cpu(cpu, start_on_cpu, &start);
	entered = ret || start.result ? 0 :
		wait_for_completion_timeout(&ctx->entered, msecs_to_jiffies(2000));
	if (entered) {
		/* Native enable callbacks run with both locks held. */
		spin_lock_irqsave(&ctx->drm.vbl_lock, flags);
		spin_lock(&ctx->drm.vblank_time_lock);
		drm_crtc_vblank_cancel_timer(crtc);
		ret = drm_crtc_vblank_start_timer(crtc);
		spin_unlock(&ctx->drm.vblank_time_lock);
		spin_unlock_irqrestore(&ctx->drm.vbl_lock, flags);
	}
	atomic_set(&ctx->release, 1);
	if (!start.result)
		hrtimer_cancel(&drm_crtc_vblank_crtc(crtc)->vblank_timer.timer);
	migrate_enable();
	cpus_read_unlock();

	KUNIT_EXPECT_EQ(test, start.result, 0);
	KUNIT_EXPECT_NE(test, entered, 0);
	KUNIT_EXPECT_EQ(test, ret, -EBUSY);
	KUNIT_EXPECT_FALSE(test, READ_ONCE(ctx->timed_out));

	/* Once the callback has left, the same timer must be reusable. */
	KUNIT_EXPECT_EQ(test, drm_crtc_vblank_start_timer(crtc), 0);
	drm_crtc_vblank_cancel_timer(crtc);
	hrtimer_cancel(&drm_crtc_vblank_crtc(crtc)->vblank_timer.timer);
}

static struct kunit_case drm_vblank_timer_tests[] = {
	KUNIT_CASE(drm_vblank_restart_does_not_wait_for_callback),
	{}
};

static struct kunit_suite drm_vblank_timer_suite = {
	.name = "drm_vblank_timer",
	.test_cases = drm_vblank_timer_tests,
};

kunit_test_suite(drm_vblank_timer_suite);

MODULE_DESCRIPTION("KUnit tests for DRM software vblank timer lifecycle");
MODULE_LICENSE("GPL");
