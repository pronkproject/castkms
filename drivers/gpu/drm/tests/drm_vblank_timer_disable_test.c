// SPDX-License-Identifier: GPL-2.0

#include <drm/drm_atomic_helper.h>
#include <drm/drm_crtc.h>
#include <drm/drm_drv.h>
#include <drm/drm_kunit_helpers.h>
#include <drm/drm_modes.h>
#include <drm/drm_modeset_helper_vtables.h>
#include <drm/drm_vblank.h>
#include <drm/drm_vblank_helper.h>

#include <kunit/test.h>
#include <linux/completion.h>
#include <linux/cpu.h>
#include <linux/hrtimer.h>
#include <linux/kthread.h>
#include <linux/sched.h>
#include <linux/smp.h>
#include <linux/workqueue.h>

struct timer_disable_device {
	struct drm_device drm;
	struct completion entered;
	struct completion off_started;
	struct completion off_done;
	struct work_struct off_work;
	struct drm_crtc *crtc;
	atomic_t release;
	atomic_t finished;
	bool timed_out;
	int timer_cpu;
	int off_cpu;
};

static bool gated_vblank(struct drm_crtc *crtc)
{
	struct timer_disable_device *ctx = container_of(crtc->dev, typeof(*ctx), drm);
	ktime_t deadline = ktime_add_ms(ktime_get(), 1000);

	ctx->timer_cpu = smp_processor_id();
	complete(&ctx->entered);
	while (!atomic_read(&ctx->release)) {
		if (ktime_after(ktime_get(), deadline)) {
			WRITE_ONCE(ctx->timed_out, true);
			break;
		}
		cpu_relax();
	}
	atomic_set(&ctx->finished, 1);
	return false;
}

static void turn_off(struct work_struct *work)
{
	struct timer_disable_device *ctx = container_of(work, typeof(*ctx), off_work);

	ctx->off_cpu = smp_processor_id();
	complete(&ctx->off_started);
	drm_crtc_vblank_off(ctx->crtc);
	complete(&ctx->off_done);
}

static const struct drm_crtc_funcs timer_funcs = {
	.reset = drm_atomic_helper_crtc_reset,
	.atomic_duplicate_state = drm_atomic_helper_crtc_duplicate_state,
	.atomic_destroy_state = drm_atomic_helper_crtc_destroy_state,
	DRM_CRTC_VBLANK_TIMER_FUNCS,
};

static const struct drm_crtc_helper_funcs timer_helpers = {
	.handle_vblank_timeout = gated_vblank,
};

static long enable_on_cpu(void *data)
{
	struct drm_crtc *crtc = data;
	int ret;

	drm_crtc_vblank_on(crtc);
	ret = drm_crtc_vblank_get(crtc);
	if (!ret)
		hrtimer_start(&drm_crtc_vblank_crtc(crtc)->vblank_timer.timer,
			      ms_to_ktime(10), HRTIMER_MODE_REL_PINNED);
	return ret;
}

static void drm_vblank_off_finishes_the_timer_callback(struct kunit *test)
{
	struct timer_disable_device *ctx;
	struct drm_plane *plane;
	struct drm_crtc *crtc;
	struct device *dev;
	unsigned long entered, off_started = 0, premature = 0;
	bool queued = false;
	int cpu, off_cpu, main_cpu, ret;

	dev = drm_kunit_helper_alloc_device(test);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, dev);
	ctx = drm_kunit_helper_alloc_drm_device(test, dev, struct timer_disable_device,
					       drm, DRIVER_MODESET);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, ctx);
	init_completion(&ctx->entered);
	init_completion(&ctx->off_started);
	init_completion(&ctx->off_done);
	INIT_WORK(&ctx->off_work, turn_off);
	atomic_set(&ctx->release, 0);
	atomic_set(&ctx->finished, 0);
	plane = drm_kunit_helper_create_primary_plane(test, &ctx->drm, NULL, NULL,
						     NULL, 0, NULL);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, plane);
	crtc = drm_kunit_helper_create_crtc(test, &ctx->drm, plane, NULL, &timer_funcs,
					  &timer_helpers);
	KUNIT_ASSERT_NOT_ERR_OR_NULL(test, crtc);
	ctx->crtc = crtc;
	KUNIT_ASSERT_EQ(test, drm_vblank_init(&ctx->drm, 1), 0);
	crtc->mode = (struct drm_display_mode) {
		DRM_MODE("1024x768", 0, 65000, 1024, 1048, 1184, 1344, 0,
			 768, 771, 777, 806, 0, DRM_MODE_FLAG_NHSYNC | DRM_MODE_FLAG_NVSYNC)
	};
	drm_mode_set_crtcinfo(&crtc->mode, 0);

	cpus_read_lock();
	migrate_disable();
	main_cpu = smp_processor_id();
	cpu = cpumask_any_but(cpu_online_mask, main_cpu);
	for_each_online_cpu(off_cpu) {
		if (off_cpu != cpu && off_cpu != main_cpu)
			break;
	}
	if (cpu >= nr_cpu_ids || off_cpu >= nr_cpu_ids) {
		migrate_enable();
		cpus_read_unlock();
		kunit_skip(test, "Requires separate CPUs for the timer, shutdown and gate owner");
		return;
	}
	/* Flushing ordinary work must not wait incidentally on the gated timer's CPU. */
	ret = set_cpus_allowed_ptr(drm_crtc_vblank_crtc(crtc)->worker->task,
				   cpumask_of(off_cpu));
	if (!ret)
		ret = work_on_cpu(cpu, enable_on_cpu, crtc);
	entered = ret ? 0 :
		wait_for_completion_timeout(&ctx->entered, msecs_to_jiffies(2000));
	if (entered) {
		queued = queue_work_on(off_cpu, system_percpu_wq, &ctx->off_work);
		off_started = wait_for_completion_timeout(&ctx->off_started,
							  msecs_to_jiffies(2000));
		if (off_started)
			premature = wait_for_completion_timeout(&ctx->off_done,
							msecs_to_jiffies(100));
	}
	atomic_set(&ctx->release, 1);
	flush_work(&ctx->off_work);
	if (!ret) {
		hrtimer_cancel(&drm_crtc_vblank_crtc(crtc)->vblank_timer.timer);
		drm_crtc_vblank_off(crtc);
		drm_crtc_vblank_put(crtc);
	}
	migrate_enable();
	cpus_read_unlock();

	KUNIT_EXPECT_EQ(test, ret, 0);
	KUNIT_EXPECT_NE(test, entered, 0);
	KUNIT_EXPECT_TRUE(test, queued);
	KUNIT_EXPECT_NE(test, off_started, 0);
	KUNIT_EXPECT_EQ(test, premature, 0);
	KUNIT_EXPECT_NE(test, ctx->timer_cpu, main_cpu);
	KUNIT_EXPECT_EQ(test, ctx->off_cpu, off_cpu);
	KUNIT_EXPECT_TRUE(test, completion_done(&ctx->off_done));
	KUNIT_EXPECT_EQ(test, atomic_read(&ctx->finished), 1);
	KUNIT_EXPECT_FALSE(test, READ_ONCE(ctx->timed_out));
}

static struct kunit_case drm_vblank_timer_disable_tests[] = {
	KUNIT_CASE(drm_vblank_off_finishes_the_timer_callback),
	{}
};

static struct kunit_suite drm_vblank_timer_disable_suite = {
	.name = "drm_vblank_timer_disable",
	.test_cases = drm_vblank_timer_disable_tests,
};

kunit_test_suite(drm_vblank_timer_disable_suite);

MODULE_DESCRIPTION("KUnit tests for DRM software vblank timer shutdown");
MODULE_LICENSE("GPL");
