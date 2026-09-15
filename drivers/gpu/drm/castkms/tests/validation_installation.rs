// SPDX-License-Identifier: GPL-2.0-only

//! Real KMS updates under injected internal validation policy, not transition UAPI tests.

use super::*;
use crate::execution::validation::Contract;

fn restrict(fixture: &Fixture, output: usize) -> Result {
    let profile = Arc::new(super::renderer_proposals::profile()?, GFP_KERNEL)?;
    fixture
        .drm
        .device()
        .validation
        .gate_for_test(output, 9, Contract::Renderer(profile))
}

#[kunit_tests(rust_castkms_validation_installation)]
mod cases {
    use super::*;

    #[test]
    fn incompatible_updates_leave_the_installed_scene_unchanged() -> Result {
        let fixture = Fixture::new()?;
        let first = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        let next = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&first, false, 0)?;
        restrict(&fixture, 0)?;
        check(fixture.select(&next, true, 0) == Err(EOPNOTSUPP))?;
        check(fixture.select(&next, false, 0) == Err(EOPNOTSUPP))?;
        check(fixture.drm.device().output.inspect(|scene| {
            scene
                .and_then(|s| s.primary())
                .is_some_and(|p| core::ptr::eq(p.framebuffer(), &*first))
        }))?;
        fixture.drm.device().validation.cancel_for_test(0, 8)?;
        check(fixture.select(&next, false, 0) == Err(EOPNOTSUPP))?;
        fixture.drm.device().validation.cancel_for_test(0, 9)?;
        fixture.select(&next, false, 0)?;
        check(fixture.drm.device().output.inspect(|scene| {
            scene
                .and_then(|s| s.primary())
                .is_some_and(|p| core::ptr::eq(p.framebuffer(), &*next))
        }))
    }

    #[test]
    fn restricted_output_can_be_explicitly_disabled() -> Result {
        let fixture = Fixture::new()?;
        let image = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&image, false, 0)?;
        restrict(&fixture, 0)?;
        fixture.drm.update(|mut transaction| {
            transaction
                .as_mut()
                .set_crtc_config(fixture.drm.crtc()?, None)
        })?;
        check(!fixture.drm.device().output.has_scene())?;
        check(fixture.select(&image, false, 0) == Err(EOPNOTSUPP))
    }

    #[test]
    fn rejecting_the_last_output_leaves_the_whole_cohort_uninstalled() -> Result {
        let fixture = Fixture::new_outputs(c"castkms-validation-cohort", 8)?;
        let image = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        let mode = DisplayMode::from_timings(ModeTimings {
            clock_khz: 25175,
            hdisplay: 640,
            hsync_start: 656,
            hsync_end: 752,
            htotal: 800,
            vdisplay: 480,
            vsync_start: 490,
            vsync_end: 492,
            vtotal: 525,
            flags: ModeFlags::NHSYNC | ModeFlags::NVSYNC,
        })?;
        let update =
            |mut transaction: Pin<&mut kernel::drm::kms::atomic::AtomicStateComposer<Driver>>| {
                for index in 0..8 {
                    transaction.as_mut().set_crtc_config(
                        fixture.drm.crtc_at(index)?,
                        Some(&CrtcScanout {
                            mode: &mode,
                            framebuffer: &image,
                            connectors: &[fixture.drm.connector_at(index)?],
                            position: (0, 0),
                        }),
                    )?;
                }
                Ok(())
            };
        let mut checked = 0;
        check(
            fixture.drm.update_after_check(update, || {
                checked += 1;
                restrict(&fixture, 7)
            }) == Err(EOPNOTSUPP),
        )?;
        check(checked == 1)?;
        for display in &fixture.drm.device().displays {
            check(!display.output.has_scene())?;
        }
        fixture.drm.check(|transaction| {
            for index in 0..8 {
                let state = transaction.add_crtc_state(fixture.drm.crtc_at(index)?)?;
                check(state.checked_scene().is_none())?;
            }
            Ok(())
        })?;
        fixture.drm.device().validation.cancel_for_test(7, 9)?;
        fixture.drm.update(update)?;
        for display in &fixture.drm.device().displays {
            check(display.output.has_scene())?;
        }
        Ok(())
    }
}
