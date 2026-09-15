// SPDX-License-Identifier: GPL-2.0-only

//! Construction bounds for the default virtual display topology.

use super::*;

#[kunit_tests(rust_castkms_topology)]
mod cases {
    use super::*;

    #[test]
    fn one_through_eight_outputs_construct() -> Result {
        for count in 1..=device::MAX_OUTPUTS {
            drop(CastKms::new_outputs(c"castkms-topology-test", count)?);
        }
        Ok(())
    }

    #[test]
    fn eight_outputs_publish_independent_scenes() -> Result {
        let fixture = Fixture::new_outputs(c"castkms-eight-scenes-test", 8)?;
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
        let mut framebuffers = KVec::new();
        for _ in 0..8 {
            framebuffers.push(
                fixture.framebuffer(provenance::Provenance::from_snapshot(None))?,
                GFP_KERNEL,
            )?;
        }
        fixture.drm.update(|mut transaction| {
            for index in 0..8 {
                let scanout = CrtcScanout {
                    mode: &mode,
                    framebuffer: &framebuffers[index],
                    connectors: &[fixture.drm.connector_at(index)?],
                    position: (0, 0),
                };
                transaction
                    .as_mut()
                    .set_crtc_config(fixture.drm.crtc_at(index)?, Some(&scanout))?;
            }
            Ok(())
        })?;
        for index in 0..8 {
            let display = &fixture.drm.device().displays[index];
            check(core::ptr::eq(
                &*fixture.drm.crtc_at(index)?.display,
                &**display,
            ))?;
            check(display.output.inspect(|scene| {
                scene
                    .and_then(|s| s.primary())
                    .is_some_and(|p| core::ptr::eq(p.framebuffer(), &*framebuffers[index]))
            }))?;
            for other in 0..index {
                check(
                    display.output.identity()
                        != fixture.drm.device().displays[other].output.identity(),
                )?;
            }
        }
        let displays = &fixture.drm.device().displays;
        let first = displays[0].startup.begin()?;
        let last = displays[7].startup.begin()?;
        first.check()?;
        last.check()?;
        fixture.drm.update(|mut transaction| {
            transaction
                .as_mut()
                .set_crtc_config(fixture.drm.crtc_at(7)?, None)
        })?;
        for (index, display) in fixture.drm.device().displays.iter().enumerate() {
            check(display.output.has_scene() == (index != 7))?;
        }
        first.check()?;
        check(last.check().is_err())?;
        check(matches!(fixture.drm.crtc_at(8), Err(EINVAL)))?;
        check(matches!(fixture.drm.plane_at(8), Err(EINVAL)))?;
        check(matches!(fixture.drm.connector_at(8), Err(EINVAL)))?;
        fixture.state.close();
        for display in &fixture.drm.device().displays {
            check(!display.output.has_scene())?;
        }
        Ok(())
    }

    #[test]
    fn zero_and_more_than_eight_outputs_are_rejected() -> Result {
        check(matches!(
            CastKms::new_outputs(c"castkms-zero-output-test", 0),
            Err(EINVAL)
        ))?;
        check(matches!(
            CastKms::new_outputs(c"castkms-too-many-output-test", device::MAX_OUTPUTS + 1),
            Err(EINVAL)
        ))
    }
}
