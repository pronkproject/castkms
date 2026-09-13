// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Counter observations use the event fixture's manually advanced clock.

use super::*;
use crate::time::{
    delay::fsleep,
    Delta, //
};

fn check(condition: bool) -> Result {
    if condition {
        Ok(())
    } else {
        Err(EINVAL)
    }
}

fn with_crtc(test: impl FnOnce(&crtc::Crtc<EventCrtc>) -> Result) -> Result {
    let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
    let parent = faux::Registration::new(c"rust-kms-vblank-sample", None)?;
    let drm = create(parent.as_ref(), &counts)?;
    // SAFETY: Setup finished and the test retains the device until the callback returns.
    let crtc = unsafe { crtc::Crtc::<EventCrtc>::from_raw(drm.crtc.load(Ordering::Relaxed)) };
    let result = test(crtc);
    crtc.vblank_off();
    drop(drm);
    drop(parent);
    result?;
    check(counts.objects.load(Ordering::Relaxed) == 0)
}

#[kunit_tests(rust_drm_vblank_samples)]
mod cases {
    use super::*;

    #[test]
    fn reading_an_unstarted_clock_does_not_enable_it() -> Result {
        with_crtc(|crtc| {
            let references = vblank_references(crtc);
            let first = crtc.vblank_count_and_time();
            let second = crtc.vblank_count_and_time();
            check(first.sequence() == second.sequence())?;
            check(first.timestamp().is_none() && second.timestamp().is_none())?;
            check(vblank_references(crtc) == references)?;
            check(
                crtc.drm_dev()
                    .observations
                    .enable_calls
                    .load(Ordering::Relaxed)
                    == 0,
            )
        })
    }

    #[test]
    fn samples_follow_the_handled_counter_and_timestamp() -> Result {
        with_crtc(|crtc| {
            crtc.vblank_on();
            let _reference = crtc.vblank_get()?;
            check(crtc.handle_vblank())?;
            let first = crtc.vblank_count_and_time();
            fsleep(Delta::from_millis(2));
            check(crtc.handle_vblank())?;
            let second = crtc.vblank_count_and_time();
            check(second.sequence() > first.sequence())?;
            let elapsed = second.timestamp().ok_or(EINVAL)? - first.timestamp().ok_or(EINVAL)?;
            check(elapsed.as_nanos() > 0)
        })
    }

    #[test]
    fn reading_a_stopped_clock_neither_restarts_nor_advances_it() -> Result {
        with_crtc(|crtc| {
            crtc.vblank_on();
            let _reference = crtc.vblank_get()?;
            check(crtc.handle_vblank())?;
            crtc.vblank_off();
            let enables = crtc
                .drm_dev()
                .observations
                .enable_calls
                .load(Ordering::Relaxed);
            let references = vblank_references(crtc);
            let first = crtc.vblank_count_and_time();
            fsleep(Delta::from_millis(2));
            let second = crtc.vblank_count_and_time();
            check(first.sequence() == second.sequence())?;
            check(!crtc.handle_vblank())?;
            check(crtc.vblank_get().err() == Some(EINVAL))?;
            check(vblank_references(crtc) == references)?;
            check(
                crtc.drm_dev()
                    .observations
                    .enable_calls
                    .load(Ordering::Relaxed)
                    == enables,
            )
        })
    }
}
