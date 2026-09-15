// SPDX-License-Identifier: GPL-2.0-only

//! Candidate replacement and shutdown do not reset outstanding snapshot storage.

use super::*;
use crate::host_compositor::{
    compose,
    layout::Layout,
    pool::Pool, //
};

fn image(fixture: &Fixture) -> Result<compose::Completed> {
    let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
    fixture.select(&fb, false, 0)?;
    let pool = Pool::new(
        fixture.drm.device(),
        &fixture.host_budget,
        Layout::new(640, 480)?,
    )?;
    compose::current(&fixture.drm.device().output, &pool)?.ok_or(EINVAL)
}

#[kunit_tests(rust_castkms_renderer_startup)]
mod cases {
    use super::*;

    #[test]
    fn releasing_an_old_candidate_cannot_cancel_its_replacement() -> Result {
        let fixture = Fixture::new()?;
        let startup = &fixture.drm.device().startup;
        let first = startup.begin()?;
        check(matches!(startup.begin(), Err(EBUSY)))?;
        first.cancel();
        let second = startup.begin()?;
        first.cancel();
        drop(first);
        check(matches!(startup.begin(), Err(EBUSY)))?;
        drop(second);
        let _third = startup.begin()?;
        check(
            fixture.drm.device().execution.describe().profile == crate::execution::Profile::HostV1,
        )?;
        check(fixture.drm.device().execution.describe().generation == 1)?;
        Ok(())
    }

    #[test]
    fn control_interval_cancellation_cannot_cancel_its_replacement() -> Result {
        let fixture = Fixture::new()?;
        let startup = &fixture.drm.device().startup;
        let first = startup.begin()?;
        startup.invalidate_current();
        check(first.check() == Err(ECANCELED))?;
        let second = startup.begin()?;
        first.cancel();
        drop(first);
        second.check()
    }

    #[test]
    fn different_outputs_have_independent_candidates() -> Result {
        let first = Fixture::new()?;
        let second = Fixture::new_named(c"castkms-startup-other")?;
        let other_image = image(&second)?;
        let a = first.drm.device().startup.begin()?;
        let b = second.drm.device().startup.begin()?;
        check(matches!(
            a.snapshot(first.drm.device(), &other_image),
            Err(EINVAL)
        ))?;
        let _copy = b.snapshot(second.drm.device(), &other_image)?;
        check(matches!(second.drm.device().startup.begin(), Err(EBUSY)))?;
        Ok(())
    }

    #[test]
    fn canceled_candidates_keep_retained_copies_charged() -> Result {
        let fixture = Fixture::new()?;
        let image = image(&fixture)?;
        let device = fixture.drm.device();
        let startup = &device.startup;
        let _pressure = startup.reserve_for_test(device, (512 - 16) * 1024 * 1024)?;
        let capacity = (16usize * 1024 * 1024)
            .checked_div(image.layout().size())
            .ok_or(EINVAL)?;
        let mut copies = KVec::new();
        for _ in 0..capacity {
            let candidate = startup.begin()?;
            copies.push(candidate.snapshot(device, &image)?, GFP_KERNEL)?;
        }
        let next = startup.begin()?;
        check(matches!(next.snapshot(device, &image), Err(EBUSY)))?;
        drop(copies.pop());
        let _replacement = next.snapshot(device, &image)?;
        drop(image);
        let host = super::image(&fixture)?;
        check(host.layout().dimensions() == (640, 480))?;
        Ok(())
    }

    #[test]
    fn cancellation_during_copy_discards_the_result_and_returns_credit() -> Result {
        let fixture = Fixture::new()?;
        let image = image(&fixture)?;
        let device = fixture.drm.device();
        for _ in 0..20 {
            let candidate = device.startup.begin()?;
            check(matches!(
                candidate.snapshot_then_for_test(device, &image, || candidate.cancel()),
                Err(ECANCELED)
            ))?;
        }
        let candidate = device.startup.begin()?;
        let _copy = candidate.snapshot(device, &image)?;
        Ok(())
    }

    #[test]
    fn shutdown_closes_candidates_without_destroying_completed_copies() -> Result {
        let fixture = Fixture::new()?;
        let image = image(&fixture)?;
        let device = fixture.drm.device();
        let startup = device.startup.clone();
        let candidate = startup.begin()?;
        let copy = candidate.snapshot(device, &image)?;
        check(matches!(
            candidate.snapshot_then_for_test(device, &image, || fixture.state.close()),
            Err(ENODEV)
        ))?;
        candidate.cancel();
        check(matches!(startup.begin(), Err(ENODEV)))?;
        check(matches!(candidate.snapshot(device, &image), Err(ENODEV)))?;
        drop(candidate);
        check(matches!(startup.begin(), Err(ENODEV)))?;
        let mut pixels = KVVec::new();
        pixels.resize(copy.layout().pixel_bytes(), 0xff, GFP_KERNEL)?;
        copy.copy_pixels(&mut pixels)?;
        check(pixels.iter().all(|byte| *byte == 0))?;
        Ok(())
    }

    #[test]
    fn dropping_registration_ownership_permanently_closes_a_retained_handle() -> Result {
        let state = device::Owner::new()?;
        let startup = state.state().startup.clone();
        let candidate = startup.begin()?;
        drop(state);
        candidate.cancel();
        check(matches!(startup.begin(), Err(ENODEV)))?;
        Ok(())
    }
}
