// SPDX-License-Identifier: GPL-2.0-only

//! Native display-clock progress and shutdown without capture or pixel reads.

use super::*;
use kernel::{
    drm::kms::{
        crtc::Crtc,
        vblank::VblankSample, //
    },
    sync::aref::ARef,
    time::{
        delay::fsleep,
        Delta,
        Instant,
        Monotonic, //
    }, //
};

fn next_sample(crtc: &Crtc<display::Crtc>, sequence: u64) -> Result<VblankSample> {
    let started = Instant::<Monotonic>::now();
    loop {
        let sample = crtc.vblank_count_and_time();
        if sample.sequence() > sequence {
            return Ok(sample);
        }
        if started.elapsed() >= Delta::from_millis(1000) {
            return Err(ETIMEDOUT);
        }
        fsleep(Delta::from_millis(5));
    }
}

fn disable(fixture: &Fixture) -> Result {
    fixture
        .drm
        .update(|transaction| transaction.set_crtc_config(fixture.drm.crtc()?, None))
}

#[kunit_tests(rust_castkms_vblank)]
mod cases {
    use super::*;

    #[test]
    fn display_ticks_without_capture_or_new_scenes() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let source = fixture.drm.device().output.inspect_accepted(|current| {
            current.map(|(source, _)| ARef::from(source)).ok_or(EINVAL)
        })?;
        let crtc = fixture.drm.crtc()?;
        let _reference = crtc.vblank_get()?;
        let initial = crtc.vblank_count_and_time();
        let first = next_sample(crtc, initial.sequence())?;
        let second = next_sample(crtc, first.sequence())?;
        let elapsed = second.timestamp().ok_or(EINVAL)? - first.timestamp().ok_or(EINVAL)?;
        check(elapsed.as_nanos() > 0)?;
        fixture.drm.device().output.inspect_accepted(|current| {
            check(current.is_some_and(|(retained, _)| core::ptr::eq(&*source, retained)))
        })
    }

    #[test]
    fn disable_stops_ticks_despite_a_retained_reference() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let owned = fixture.drm.crtc()?.vblank_get()?.into_owned();
        let before = owned.crtc().vblank_count_and_time();
        next_sample(owned.crtc(), before.sequence())?;
        disable(&fixture)?;
        let stopped = owned.crtc().vblank_count_and_time();
        fsleep(Delta::from_millis(40));
        check(owned.crtc().vblank_count_and_time().sequence() == stopped.sequence())?;
        check(owned.crtc().vblank_get().err() == Some(EINVAL))?;
        check(fixture.drm.device().output.inspect(|scene| scene.is_none()))
    }

    #[test]
    fn repeated_modesets_restart_the_clock_with_a_retained_reference() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let owned = fixture.drm.crtc()?.vblank_get()?.into_owned();
        for _ in 0..16 {
            let before = owned.crtc().vblank_count_and_time();
            next_sample(owned.crtc(), before.sequence())?;
            disable(&fixture)?;
            fixture.select(&fb, false, 0)?;
        }
        let restarted = owned.crtc().vblank_count_and_time();
        next_sample(owned.crtc(), restarted.sequence())?;
        Ok(())
    }

    #[test]
    fn owned_reference_remains_readable_after_atomic_shutdown() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let owned = fixture.drm.crtc()?.vblank_get()?.into_owned();
        drop(fb);
        // Keep the faux parent bound until the final retained DRM reference is released.
        let Fixture {
            state,
            host_budget,
            drm,
            _parent,
        } = fixture;
        drop(state);
        drop(drm);
        drop(host_budget);
        let stopped = owned.crtc().vblank_count_and_time();
        fsleep(Delta::from_millis(40));
        let stable = owned.crtc().vblank_count_and_time().sequence() == stopped.sequence();
        let refused = owned.crtc().vblank_get().err() == Some(EINVAL);
        drop(owned);
        drop(_parent);
        check(stable && refused)
    }
}
