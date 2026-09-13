// SPDX-License-Identifier: GPL-2.0-only

//! Attribution of active blank outputs is independent of retained plane storage.

use super::*;

fn blank(fixture: &Fixture, check_only: bool) -> Result {
    let edit =
        |mut transaction: Pin<&mut kernel::drm::kms::atomic::AtomicStateComposer<Driver>>| {
            transaction.as_mut().disable_plane(fixture.drm.plane()?)?;
            drop(transaction.add_crtc_state(fixture.drm.crtc()?)?);
            Ok(())
        };
    if check_only {
        fixture.drm.check(edit)
    } else {
        fixture.drm.update(edit)
    }
}

fn is_blank(fixture: &Fixture) -> bool {
    fixture.drm.device().output.inspect(|scene| {
        scene.is_some_and(|scene| scene.primary().is_none() && scene.content_serial().is_none())
    })
}

fn owner(fixture: &Fixture) -> Result<MasterRef<Driver>> {
    let owner = fixture
        .drm
        .synthetic_master_snapshot(true)?
        .master()
        .clone();
    fixture.drm.device().authority.changed(Some(owner.clone()));
    Ok(owner)
}

#[kunit_tests(rust_castkms_blank_ownership)]
mod cases {
    use super::*;

    #[test]
    fn initial_blank_activation_adopts_the_current_master() -> Result {
        let fixture = Fixture::new()?;
        let previous = fixture.drm.synthetic_master_snapshot(true)?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(Some(previous)))?;
        let current = owner(&fixture)?;
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
        let scanout = CrtcScanout {
            mode: &mode,
            framebuffer: &fb,
            connectors: &[fixture.drm.connector()?],
            position: (0, 0),
        };
        let install =
            |mut transaction: Pin<&mut kernel::drm::kms::atomic::AtomicStateComposer<Driver>>| {
                transaction
                    .as_mut()
                    .set_crtc_config(fixture.drm.crtc()?, Some(&scanout))?;
                transaction.disable_plane(fixture.drm.plane()?)
            };
        fixture.drm.check(install)?;
        check(!fixture.drm.device().output.has_scene())?;
        fixture.drm.update(install)?;
        check(is_blank(&fixture))?;
        check(fixture.has_owner(Some(&current)))?;
        check(
            fixture
                .drm
                .device()
                .output
                .inspect(|scene| scene.is_some_and(|scene| scene.producer_result() == Ok(()))),
        )?;
        Ok(())
    }

    #[test]
    fn removing_visible_content_adopts_the_current_blank_owner() -> Result {
        let fixture = Fixture::new()?;
        let first = owner(&fixture)?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        check(fixture.has_owner(Some(&first)))?;
        let next = owner(&fixture)?;
        blank(&fixture, false)?;
        check(is_blank(&fixture))?;
        check(fixture.has_owner(Some(&next)))?;
        check(
            fixture
                .drm
                .device()
                .output
                .inspect(|scene| scene.is_some_and(|scene| scene.producer_result() == Ok(()))),
        )?;
        fixture.select(&fb, false, 0)?;
        check(!is_blank(&fixture))?;
        check(fixture.has_owner(Some(&next)))?;
        Ok(())
    }

    #[test]
    fn a_blank_noop_does_not_adopt_a_replacement_master() -> Result {
        let fixture = Fixture::new()?;
        let first = owner(&fixture)?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        blank(&fixture, false)?;
        let _next = owner(&fixture)?;
        blank(&fixture, false)?;
        check(is_blank(&fixture))?;
        check(fixture.has_owner(Some(&first)))?;
        fixture.drm.update(|transaction| {
            drop(transaction.add_crtc_state(fixture.drm.crtc()?)?);
            Ok(())
        })?;
        check(fixture.has_owner(Some(&first)))?;
        Ok(())
    }

    #[test]
    fn a_modeset_of_an_active_blank_adopts_the_current_master() -> Result {
        let fixture = Fixture::new()?;
        let _first = owner(&fixture)?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        blank(&fixture, false)?;
        let next = owner(&fixture)?;
        fixture.drm.update(|transaction| {
            transaction
                .add_crtc_state(fixture.drm.crtc()?)?
                .set_mode_changed(true);
            Ok(())
        })?;
        check(is_blank(&fixture))?;
        check(fixture.has_owner(Some(&next)))?;
        Ok(())
    }

    #[test]
    fn checking_or_rejecting_a_blank_does_not_publish_its_owner() -> Result {
        let fixture = Fixture::new()?;
        let first = owner(&fixture)?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let _next = owner(&fixture)?;
        blank(&fixture, true)?;
        check(!is_blank(&fixture))?;
        check(fixture.has_owner(Some(&first)))?;
        check(
            fixture.drm.update(|mut transaction| {
                transaction.as_mut().disable_plane(fixture.drm.plane()?)?;
                Err(EINVAL)
            }) == Err(EINVAL),
        )?;
        check(!is_blank(&fixture))?;
        check(fixture.has_owner(Some(&first)))?;
        Ok(())
    }

    #[test]
    fn unknown_blank_ownership_survives_a_noop_with_a_new_master() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        blank(&fixture, false)?;
        let _next = owner(&fixture)?;
        blank(&fixture, false)?;
        check(is_blank(&fixture))?;
        check(fixture.has_owner(None))?;
        Ok(())
    }

    #[test]
    fn disabling_the_controller_removes_the_blank_scene() -> Result {
        let fixture = Fixture::new()?;
        let _first = owner(&fixture)?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        blank(&fixture, false)?;
        fixture
            .drm
            .update(|transaction| transaction.set_crtc_config(fixture.drm.crtc()?, None))?;
        check(!fixture.drm.device().output.has_scene())?;
        let next = owner(&fixture)?;
        fixture.select(&fb, false, 0)?;
        blank(&fixture, false)?;
        check(is_blank(&fixture))?;
        check(fixture.has_owner(Some(&next)))?;
        Ok(())
    }
}
