// SPDX-License-Identifier: GPL-2.0-only

//! Real atomic multi-plane updates and the resulting privately composed pixels.

use super::*;
use crate::host_compositor::{compose, layout::Layout, pool::Pool};
use kernel::{
    drm::kms::{atomic::PlaneScanout, crtc::ColorLut},
    io::{io_project, Io},
};

fn small_image(fixture: &Fixture, pixel: u32) -> Result<FramebufferRef<Driver>> {
    let object = shmem::Object::<gem::Object>::new(
        fixture.drm.device(),
        4096,
        Default::default(),
        Default::default(),
    )?;
    {
        let map = object.vmap::<0>()?;
        for offset in [0, 4, 8, 12] {
            map.try_write32(pixel, offset)?;
        }
    }
    fixture.drm.framebuffer(
        &FramebufferLayout {
            width: 2,
            height: 2,
            format: drm::fourcc::ARGB8888,
            modifier: Some(drm::fourcc::FORMAT_MOD_LINEAR),
            interlaced: false,
            planes: &[FramebufferPlane {
                object: &object,
                pitch: 8,
                offset: 0,
            }],
        },
        provenance::Provenance::from_snapshot(None),
    )
}

fn configure(
    fixture: &Fixture,
    index: usize,
    image: &FramebufferRef<Driver>,
    position: [i32; 2],
    destination: [u32; 2],
) -> Result {
    fixture.drm.update(|mut transaction| {
        transaction.as_mut().set_plane_config(
            fixture.drm.plane_at(index)?,
            &PlaneScanout {
                crtc: fixture.drm.crtc()?,
                framebuffer: image,
                source: [0, 0, image.width() << 16, image.height() << 16],
                position,
                destination,
            },
        )
    })
}

fn pixels(fixture: &Fixture, pool: &Arc<Pool>) -> Result<[u32; 4]> {
    let image = compose::current(&fixture.drm.device().output, pool)?.ok_or(EINVAL)?;
    let mut row = [0; 2560];
    image.read_row(0, &mut row)?;
    Ok(core::array::from_fn(|index| {
        u32::from_le_bytes(row[index * 4..index * 4 + 4].try_into().unwrap())
    }))
}

#[kunit_tests(rust_castkms_composition_planes)]
mod cases {
    use super::*;

    #[test]
    fn cursor_and_overlay_updates_preserve_the_primary() -> Result {
        let fixture = Fixture::new_features(c"castkms-composition-planes", 1, true, true)?;
        let primary = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        {
            let map = primary.vmap::<gem::Object>()?;
            for offset in [0, 4, 8, 12] {
                io_project!(map.view(), [try: offset..offset + 4])
                    .copy_from_slice(&0x000000ffu32.to_le_bytes());
            }
        }
        fixture.select(&primary, false, 0)?;
        let pool = Pool::new(
            fixture.drm.device(),
            &fixture.host_budget,
            Layout::new(640, 480)?,
        )?;
        let overlay = small_image(&fixture, 0x80800000)?;
        configure(&fixture, 2, &overlay, [1, 0], [4, 2])?;
        check(pixels(&fixture, &pool)? == [0xff, 0x80007f, 0x80007f, 0x80007f])?;
        let cursor = small_image(&fixture, 0xffffffff)?;
        configure(&fixture, 1, &cursor, [2, 0], [2, 2])?;
        check(pixels(&fixture, &pool)? == [0xff, 0x80007f, 0xffffff, 0xffffff])?;
        fixture.drm.update(|mut transaction| {
            transaction.as_mut().disable_plane(fixture.drm.plane_at(1)?)
        })?;
        check(pixels(&fixture, &pool)? == [0xff, 0x80007f, 0x80007f, 0x80007f])?;
        configure(&fixture, 2, &overlay, [-1, -1], [2, 2])?;
        check(pixels(&fixture, &pool)? == [0x80007f, 0xff, 0xff, 0xff])?;
        fixture.drm.update(|mut transaction| {
            transaction.as_mut().disable_plane(fixture.drm.plane_at(2)?)
        })?;
        check(pixels(&fixture, &pool)? == [0xff; 4])?;
        Ok(())
    }

    #[test]
    fn gamma_only_updates_recompose_unchanged_sources() -> Result {
        let fixture = Fixture::new()?;
        let primary = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&primary, false, 0)?;
        let pool = Pool::new(
            fixture.drm.device(),
            &fixture.host_budget,
            Layout::new(640, 480)?,
        )?;
        fixture.drm.update(|transaction| {
            transaction
                .add_crtc_state(fixture.drm.crtc()?)?
                .set_gamma_lut(Some(&[ColorLut::new(65535, 0, 0)]))
        })?;
        check(pixels(&fixture, &pool)? == [0xff0000; 4])?;
        fixture.drm.update(|transaction| {
            transaction
                .add_crtc_state(fixture.drm.crtc()?)?
                .set_gamma_lut(None)
        })?;
        check(pixels(&fixture, &pool)? == [0; 4])?;
        Ok(())
    }

    #[test]
    fn eight_outputs_have_eight_shared_overlays_and_eight_cursors() -> Result {
        let fixture = Fixture::new_features(c"castkms-full-plane-topology", 8, true, true)?;
        fixture.drm.plane_at(23)?;
        check(matches!(fixture.drm.plane_at(24), Err(EINVAL)))?;
        fixture.drm.crtc_at(7)?;
        Ok(())
    }

    #[test]
    fn color_only_plane_updates_apply_the_selected_matrices() -> Result {
        use kernel::drm::kms::colorop::Operation;
        let fixture = Fixture::new_features(c"castkms-plane-color", 1, true, true)?;
        let primary = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&primary, false, 0)?;
        let overlay = small_image(&fixture, 0xffff0000)?;
        configure(&fixture, 2, &overlay, [0, 0], [2, 2])?;
        let pool = Pool::new(
            fixture.drm.device(),
            &fixture.host_budget,
            Layout::new(640, 480)?,
        )?;
        let mut swap = [0; 12];
        swap[2] = 1 << 32;
        swap[5] = 1 << 32;
        swap[8] = 1 << 32;
        fixture.drm.update(|transaction| {
            let mut state = transaction.add_plane_state(fixture.drm.plane_at(2)?)?;
            state.select_color_pipeline(Some(0))?;
            state.set_color_operation(1, Operation::Matrix(swap))
        })?;
        check(pixels(&fixture, &pool)? == [0xff, 0xff, 0, 0])?;
        fixture.drm.update(|transaction| {
            transaction
                .add_plane_state(fixture.drm.plane_at(2)?)?
                .set_color_operation(2, Operation::Matrix(swap))
        })?;
        check(pixels(&fixture, &pool)? == [0xff0000, 0xff0000, 0, 0])?;
        fixture.drm.update(|transaction| {
            transaction
                .add_plane_state(fixture.drm.plane_at(2)?)?
                .select_color_pipeline(None)
        })?;
        check(pixels(&fixture, &pool)? == [0xff0000, 0xff0000, 0, 0])?;
        Ok(())
    }

    #[test]
    fn stacking_changes_and_invalid_positions_are_atomic() -> Result {
        let fixture = Fixture::new_features(c"castkms-overlay-stacking", 1, true, true)?;
        let primary = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&primary, false, 0)?;
        let red = small_image(&fixture, 0xffff0000)?;
        let green = small_image(&fixture, 0xff00ff00)?;
        configure(&fixture, 2, &red, [0, 0], [2, 2])?;
        configure(&fixture, 3, &green, [0, 0], [2, 2])?;
        let pool = Pool::new(
            fixture.drm.device(),
            &fixture.host_budget,
            Layout::new(640, 480)?,
        )?;
        check(pixels(&fixture, &pool)? == [0xff00, 0xff00, 0, 0])?;
        fixture.drm.update(|transaction| {
            transaction
                .add_plane_state(fixture.drm.plane_at(2)?)?
                .set_zpos(2)
        })?;
        check(pixels(&fixture, &pool)? == [0xff0000, 0xff0000, 0, 0])?;
        check(
            fixture.drm.update(|transaction| {
                transaction
                    .add_plane_state(fixture.drm.plane_at(2)?)?
                    .set_zpos(31)
            }) == Err(EINVAL),
        )?;
        check(pixels(&fixture, &pool)? == [0xff0000, 0xff0000, 0, 0])?;
        Ok(())
    }

    #[test]
    fn output_color_order_is_degamma_matrix_gamma() -> Result {
        use kernel::drm::kms::crtc::ColorCtm;
        let fixture = Fixture::new()?;
        let primary = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&primary, false, 0)?;
        let pool = Pool::new(
            fixture.drm.device(),
            &fixture.host_budget,
            Layout::new(640, 480)?,
        )?;
        fixture.drm.update(|transaction| {
            let mut state = transaction.add_crtc_state(fixture.drm.crtc()?)?;
            state.set_degamma_lut(Some(&[ColorLut::new(65535, 0, 0)]))?;
            let swap = ColorCtm::from_raw([0, 0, 1 << 32, 0, 1 << 32, 0, 1 << 32, 0, 0]);
            state.set_ctm(Some(&swap))?;
            state.set_gamma_lut(Some(&[
                ColorLut::new(0, 0, 0),
                ColorLut::new(65535, 65535, 32768),
            ]))
        })?;
        check(pixels(&fixture, &pool)? == [0x80; 4])?;
        Ok(())
    }

    #[test]
    fn shared_overlay_can_be_reassigned_after_disable() -> Result {
        let fixture = Fixture::new_features(c"castkms-overlay-routing", 2, true, true)?;
        let primary = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
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
        fixture.drm.update(|mut transaction| {
            for index in 0..2 {
                transaction.as_mut().set_crtc_config(
                    fixture.drm.crtc_at(index)?,
                    Some(&CrtcScanout {
                        mode: &mode,
                        framebuffer: &primary,
                        connectors: &[fixture.drm.connector_at(index)?],
                        position: (0, 0),
                    }),
                )?;
            }
            Ok(())
        })?;
        let overlay = small_image(&fixture, 0xffff0000)?;
        for target in [1, 0] {
            fixture.drm.update(|mut transaction| {
                transaction.as_mut().set_plane_config(
                    fixture.drm.plane_at(4)?,
                    &PlaneScanout {
                        crtc: fixture.drm.crtc_at(target)?,
                        framebuffer: &overlay,
                        source: [0, 0, 2 << 16, 2 << 16],
                        position: [0, 0],
                        destination: [2, 2],
                    },
                )
            })?;
            for index in 0..2 {
                check(
                    fixture.drm.device().displays[index]
                        .output
                        .inspect(|scene| {
                            scene.is_some_and(|scene| {
                                scene.layers().count() == if index == target { 2 } else { 1 }
                            })
                        }),
                )?;
            }
            fixture.drm.update(|mut transaction| {
                transaction.as_mut().disable_plane(fixture.drm.plane_at(4)?)
            })?;
        }
        Ok(())
    }

    #[test]
    fn mixed_plane_owners_cannot_relabel_retained_content() -> Result {
        let fixture = Fixture::new_features(c"castkms-plane-owners", 1, true, true)?;
        let first = fixture
            .drm
            .synthetic_master_snapshot(true)?
            .master()
            .clone();
        fixture.drm.device().authority.changed(Some(first.clone()));
        let primary = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&primary, false, 0)?;
        check(fixture.has_owner(Some(&first)))?;
        let second = fixture
            .drm
            .synthetic_master_snapshot(true)?
            .master()
            .clone();
        fixture.drm.device().authority.changed(Some(second));
        let overlay = small_image(&fixture, 0xffff0000)?;
        configure(&fixture, 2, &overlay, [0, 0], [2, 2])?;
        check(fixture.has_owner(None))?;
        let pool = Pool::new(
            fixture.drm.device(),
            &fixture.host_budget,
            Layout::new(640, 480)?,
        )?;
        check(
            compose::current(&fixture.drm.device().output, &pool)?
                .ok_or(EINVAL)?
                .owner()
                .is_none(),
        )?;
        fixture.drm.update(|mut transaction| {
            transaction.as_mut().disable_plane(fixture.drm.plane_at(2)?)
        })?;
        check(fixture.has_owner(Some(&first)))?;
        Ok(())
    }

}
