// SPDX-License-Identifier: GPL-2.0-only

//! Construction bounds for the default virtual display topology.

use super::*;

#[kunit_tests(rust_castkms_topology)]
mod cases {
    use super::*;
    use kernel::drm::kms::plane::RawPlane;

    #[test]
    fn allocation_descriptions_use_each_outputs_actual_planes() -> Result {
        for (count, cursor, overlay) in [
            (1, false, false),
            (2, true, false),
            (2, false, true),
            (8, true, true),
        ] {
            let fixture =
                Fixture::new_features(c"castkms-allocation-topology", count, cursor, overlay)?;
            let stride = 1 + usize::from(cursor);
            for index in 0..count as usize {
                let crtc = fixture.drm.crtc_at(index)?;
                let topology = crtc.allocation_topology()?;
                let planes = topology.planes();
                check(planes.len() == stride + if overlay { 8 } else { 0 })?;
                check(planes[0].id == fixture.drm.plane_at(index * stride)?.object_id())?;
                check(planes[0].kind == scene::Kind::Primary)?;
                if cursor {
                    check(planes[1].id == fixture.drm.plane_at(index * stride + 1)?.object_id())?;
                    check(planes[1].kind == scene::Kind::Cursor)?;
                }
                if overlay {
                    for offset in 0..8 {
                        check(
                            planes[stride + offset].id
                                == fixture
                                    .drm
                                    .plane_at(count as usize * stride + offset)?
                                    .object_id(),
                        )?;
                        check(planes[stride + offset].kind == scene::Kind::Overlay)?;
                    }
                }
                let description = execution::constraints::host(planes, &[])?;
                check(
                    description
                        .formats()
                        .iter()
                        .all(|format| planes.iter().any(|plane| plane.id == format.plane_id())),
                )?;
                for other in 0..count as usize {
                    if other != index {
                        let other_primary = fixture.drm.plane_at(other * stride)?.object_id();
                        check(!planes.iter().any(|plane| plane.id == other_primary))?;
                    }
                }
            }
        }
        Ok(())
    }

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
        fixture.drm.update(|mut transaction| {
            transaction
                .as_mut()
                .set_crtc_config(fixture.drm.crtc_at(7)?, None)
        })?;
        for (index, display) in fixture.drm.device().displays.iter().enumerate() {
            check(display.output.has_scene() == (index != 7))?;
        }
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
