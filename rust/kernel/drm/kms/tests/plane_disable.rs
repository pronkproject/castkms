// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Plane-only disable through a kernel transaction, with no userspace role or descriptor.

use super::*;
use crtc::RawCrtcState;
use plane::RawPlaneState;

#[track_caller]
fn check(condition: bool) -> Result {
    if condition {
        Ok(())
    } else {
        let location = core::panic::Location::caller();
        pr_err!(
            "Plane disable check failed at {}:{}\n",
            location.file(),
            location.line()
        );
        Err(EINVAL)
    }
}

#[kunit_tests(rust_drm_plane_disable)]
mod cases {
    use super::*;

    #[test]
    fn test_only_disable_records_assignment_without_changing_display() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-plane-disable-check", None)?;
        let fixture = testing::TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        let fb = framebuffer(fixture.device())?;
        let mode = mode()?;
        let scanout = atomic::CrtcScanout {
            mode: &mode,
            framebuffer: &fb,
            connectors: &[fixture.connector()?],
            position: (0, 0),
        };
        fixture
            .update(|transaction| transaction.set_crtc_config(fixture.crtc()?, Some(&scanout)))?;
        fixture.check(|mut transaction| {
            transaction.as_mut().disable_plane(fixture.plane()?)?;
            let state = transaction.add_plane_state(fixture.plane()?)?;
            check(state.framebuffer_was_set())?;
            check(state.framebuffer::<TestDriver>().is_none())?;
            check(state.crtc::<TestDriver>().is_none())?;
            check(state.crtc_w() == 0 && state.crtc_h() == 0)?;
            check(state.source_width_16_16() == 0 && state.source_height_16_16() == 0)?;
            drop(state);
            check(transaction.add_crtc_state(fixture.crtc()?)?.active())
        })?;
        fixture.check(|transaction| {
            let state = transaction.add_plane_state(fixture.plane()?)?;
            check(state.framebuffer::<TestDriver>().is_some())?;
            check(!state.framebuffer_was_set())?;
            check(transaction.add_crtc_state(fixture.crtc()?)?.active())
        })?;
        check(counts.disables.load(Ordering::Relaxed) == 0)?;
        Ok(())
    }

    #[test]
    fn accepted_disable_releases_the_framebuffer_but_keeps_the_crtc_active() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-plane-disable-commit", None)?;
        let fixture = testing::TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        let fb = framebuffer(fixture.device())?;
        let mode = mode()?;
        let scanout = atomic::CrtcScanout {
            mode: &mode,
            framebuffer: &fb,
            connectors: &[fixture.connector()?],
            position: (0, 0),
        };
        fixture
            .update(|transaction| transaction.set_crtc_config(fixture.crtc()?, Some(&scanout)))?;
        fixture.update(|transaction| transaction.disable_plane(fixture.plane()?))?;
        drop(fb);
        check(counts.gem_objects.load(Ordering::Relaxed) == 0)?;
        fixture.check(|transaction| {
            check(
                transaction
                    .add_plane_state(fixture.plane()?)?
                    .framebuffer::<TestDriver>()
                    .is_none(),
            )?;
            check(transaction.add_crtc_state(fixture.crtc()?)?.active())
        })?;
        check(counts.disables.load(Ordering::Relaxed) == 0)?;
        fixture.update(|transaction| transaction.set_crtc_config(fixture.crtc()?, None))?;
        check(counts.disables.load(Ordering::Relaxed) == 1)?;
        Ok(())
    }

    #[test]
    fn a_plane_from_another_device_is_rejected_before_state_access() -> Result {
        let counts = Arc::new(Counts::default(), GFP_KERNEL)?;
        let parent = faux::Registration::new(c"rust-plane-disable-device", None)?;
        let fixture = testing::TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        let foreign = testing::TestDevice::new(allocate(parent.as_ref(), &counts, false)?)?;
        fixture.check(|mut transaction| {
            check(transaction.as_mut().disable_plane(foreign.plane()?) == Err(EINVAL))?;
            check(transaction.get_new_plane_state(fixture.plane()?).is_none())
        })?;
        Ok(())
    }
}
