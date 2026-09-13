// SPDX-License-Identifier: GPL-2.0-only

//! Mode and routing intervals published by real atomic callbacks.

use super::*;
use kernel::drm::kms::connector::RawConnector;

fn accepted(fixture: &Fixture) -> Result<scene::Configuration> {
    fixture
        .drm
        .device()
        .output
        .with_accepted(|accepted| accepted.and_then(|accepted| accepted.configuration.clone()))
        .ok_or(EINVAL)
}

fn select_clock(
    fixture: &Fixture,
    framebuffer: &FramebufferRef<Driver>,
    clock_khz: i32,
    check_only: bool,
) -> Result {
    let mode = DisplayMode::from_timings(ModeTimings {
        clock_khz,
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
        framebuffer,
        connectors: &[fixture.drm.connector()?],
        position: (0, 0),
    };
    let update =
        |mut transaction: Pin<&mut kernel::drm::kms::atomic::AtomicStateComposer<Driver>>| {
            transaction
                .as_mut()
                .set_crtc_config(fixture.drm.crtc()?, Some(&scanout))
        };
    if check_only {
        fixture.drm.check(update)
    } else {
        fixture.drm.update(update)
    }
}

#[kunit_tests(rust_castkms_configurations)]
mod cases {
    use super::*;

    #[test]
    fn accepted_configuration_describes_the_selected_connector_and_mode() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        check(accepted(&fixture).is_err())?;
        fixture.select(&fb, false, 0)?;
        let configuration = accepted(&fixture)?;
        check(configuration.connector_mask() == fixture.drm.connector()?.mask())?;
        check(configuration.dimensions() == [640, 480])?;
        Ok(())
    }

    #[test]
    fn scene_updates_do_not_end_the_configuration_interval() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let first = accepted(&fixture)?;
        let content = fixture
            .drm
            .device()
            .output
            .inspect(|scene| scene.map(scene::Scene::content_serial));
        fixture.select(&fb, false, 0)?;
        check(accepted(&fixture)? == first)?;
        check(
            fixture
                .drm
                .device()
                .output
                .inspect(|scene| scene.map(scene::Scene::content_serial) != content),
        )?;
        let replacement = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&replacement, false, 0)?;
        check(accepted(&fixture)? == first)?;
        fixture.drm.update(|transaction| {
            drop(transaction.add_crtc_state(fixture.drm.crtc()?)?);
            Ok(())
        })?;
        check(accepted(&fixture)? == first)?;
        Ok(())
    }

    #[test]
    fn changing_timings_starts_a_new_interval_at_the_same_dimensions() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let first = accepted(&fixture)?;
        select_clock(&fixture, &fb, 25200, false)?;
        let second = accepted(&fixture)?;
        check(second != first)?;
        check(second.dimensions() == first.dimensions())?;
        check(second.connector_mask() == first.connector_mask())?;
        select_clock(&fixture, &fb, 25200, false)?;
        check(accepted(&fixture)? == second)?;
        Ok(())
    }

    #[test]
    fn unaccepted_modes_do_not_replace_the_published_interval() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let first = accepted(&fixture)?;
        select_clock(&fixture, &fb, 25200, true)?;
        check(accepted(&fixture)? == first)?;
        check(fixture.select(&fb, false, 1).is_err())?;
        check(accepted(&fixture)? == first)?;
        Ok(())
    }

    #[test]
    fn disable_and_reenable_do_not_reuse_configuration_identity() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let first = accepted(&fixture)?;
        fixture.drm.update(|mut transaction| {
            transaction
                .as_mut()
                .set_crtc_config(fixture.drm.crtc()?, None)
        })?;
        check(accepted(&fixture).is_err())?;
        fixture.select(&fb, false, 0)?;
        check(accepted(&fixture)? != first)?;
        Ok(())
    }

    #[test]
    fn retained_configuration_survives_shutdown_without_staying_current() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let first = accepted(&fixture)?;
        let retained = first.clone();
        fixture.state.close();
        check(accepted(&fixture).is_err())?;
        check(retained == first)?;
        check(retained.dimensions() == [640, 480])?;
        Ok(())
    }
}
