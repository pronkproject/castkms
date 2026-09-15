// SPDX-License-Identifier: GPL-2.0-only

//! Complete validation metadata belongs to atomic state, not output publication.

use super::*;
use kernel::drm::kms::atomic::PlaneScanout;

fn installed(fixture: &Fixture) -> Result<Option<scene::Scene>> {
    let mut observed = None;
    fixture.drm.check(|transaction| {
        let state = transaction.add_crtc_state(fixture.drm.crtc()?)?;
        observed = Some(state.checked_scene().cloned());
        Ok(())
    })?;
    observed.ok_or(EINVAL)
}

fn overlay(fixture: &Fixture, image: &FramebufferRef<Driver>, x: i32) -> Result {
    fixture.drm.update(|mut transaction| {
        transaction.as_mut().set_plane_config(
            fixture.drm.plane_at(2)?,
            &PlaneScanout {
                crtc: fixture.drm.crtc()?,
                framebuffer: image,
                source: [0, 0, 640 << 16, 480 << 16],
                position: [x, 0],
                destination: [640, 480],
            },
        )
    })
}

#[kunit_tests(rust_castkms_atomic_scenes)]
mod cases {
    use super::*;

    #[test]
    fn partial_updates_retain_unchanged_layer_metadata() -> Result {
        let fixture = Fixture::new_features(c"castkms-atomic-scene", 1, true, true)?;
        let primary = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        let image = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&primary, false, 0)?;
        overlay(&fixture, &image, 3)?;
        let first = installed(&fixture)?.ok_or(EINVAL)?;
        check(first.layers().count() == 2)?;
        check(core::ptr::eq(
            first.primary().ok_or(EINVAL)?.framebuffer(),
            &*primary,
        ))?;
        check(first.layers().all(|layer| layer.producer.is_none()))?;
        overlay(&fixture, &image, 7)?;
        let second = installed(&fixture)?.ok_or(EINVAL)?;
        check(second.layers().count() == 2)?;
        check(second.layers().nth(1).ok_or(EINVAL)?.geometry().position == [7, 0])?;
        check(second.content_serial() != first.content_serial())?;
        fixture.drm.update(|mut transaction| {
            transaction.as_mut().disable_plane(fixture.drm.plane_at(2)?)
        })?;
        check(installed(&fixture)?.ok_or(EINVAL)?.layers().count() == 1)
    }

    #[test]
    fn test_only_does_not_replace_the_installed_description() -> Result {
        let fixture = Fixture::new()?;
        let first = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        let second = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&first, false, 0)?;
        let before = installed(&fixture)?.ok_or(EINVAL)?;
        fixture.select(&second, true, 0)?;
        let after = installed(&fixture)?.ok_or(EINVAL)?;
        check(before.content_serial() == after.content_serial())?;
        check(core::ptr::eq(
            after.primary().ok_or(EINVAL)?.framebuffer(),
            &*first,
        ))
    }

    #[test]
    fn scene_construction_does_not_depend_on_published_output() -> Result {
        let fixture = Fixture::new_features(c"castkms-scene-no-publication", 1, true, true)?;
        let primary = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        let image = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&primary, false, 0)?;
        fixture.drm.device().output.close();
        overlay(&fixture, &image, 3)?;
        check(!fixture.drm.device().output.has_scene())?;
        let scene = installed(&fixture)?.ok_or(EINVAL)?;
        check(scene.layers().count() == 2)?;
        check(core::ptr::eq(
            scene.primary().ok_or(EINVAL)?.framebuffer(),
            &*primary,
        ))
    }

    #[test]
    fn blank_and_disabled_outputs_have_distinct_descriptions() -> Result {
        let fixture = Fixture::new()?;
        let image = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&image, false, 0)?;
        fixture
            .drm
            .update(|mut transaction| transaction.as_mut().disable_plane(fixture.drm.plane()?))?;
        check(installed(&fixture)?.ok_or(EINVAL)?.layers().count() == 0)?;
        fixture.drm.update(|mut transaction| {
            transaction
                .as_mut()
                .set_crtc_config(fixture.drm.crtc()?, None)
        })?;
        check(installed(&fixture)?.is_none())
    }

    #[test]
    fn output_color_only_updates_refresh_validation_metadata() -> Result {
        use kernel::drm::kms::crtc::ColorLut;
        let fixture = Fixture::new()?;
        let image = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&image, false, 0)?;
        fixture.drm.update(|transaction| {
            transaction
                .add_crtc_state(fixture.drm.crtc()?)?
                .set_gamma_lut(Some(&[ColorLut::new(1, 2, 3)]))
        })?;
        let scene = installed(&fixture)?.ok_or(EINVAL)?;
        check(scene.layers().count() == 1)?;
        let (_, _, gamma) = scene.output_color.as_ref().ok_or(EINVAL)?.description();
        check(gamma == Some(&[[1, 2, 3]][..]))
    }
}
