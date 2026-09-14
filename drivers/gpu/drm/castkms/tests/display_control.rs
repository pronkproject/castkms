// SPDX-License-Identifier: GPL-2.0-only

//! Display control does not imply ownership of the current image.

use super::*;
use crate::display_control::Target;
use kernel::{
    drm::{
        kms::testing::MasterFile,
        preparation::Source, //
    },
    sync::aref::ARef, //
};

fn target(fixture: &Fixture, file: &MasterFile<'_, Driver>) -> Result<Target> {
    let snapshot = file.file().master_snapshot().ok_or(EINVAL)?;
    let guard = snapshot.master().lock_current().ok_or(EACCES)?;
    Target::new(&guard, fixture.drm.crtc()?, fixture.drm.connector()?)
}

#[kunit_tests(rust_castkms_display_control)]
mod cases {
    use super::*;

    #[test]
    fn generation_matching_does_not_use_framebuffer_or_configuration_equality() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let target = target(&fixture, &file)?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let first: ARef<Source> = fixture
            .drm
            .device()
            .output
            .with_accepted(|accepted| accepted.map(|accepted| accepted.source.into()))
            .ok_or(EINVAL)?;
        target.with_current(|current| current.check_source(&first))?;
        fixture.select(&fb, false, 0)?;
        let second: ARef<Source> = fixture
            .drm
            .device()
            .output
            .with_accepted(|accepted| accepted.map(|accepted| accepted.source.into()))
            .ok_or(EINVAL)?;
        target.with_current(|current| {
            check(current.configuration().dimensions() == [640, 480])?;
            check(current.check_source(&first) == Err(EAGAIN))?;
            current.check_source(&second)
        })?;
        Ok(())
    }

    #[test]
    fn installed_control_rejects_a_registration_for_another_device() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let target = target(&fixture, &file)?;
        let other = CastKms::new(c"castkms-control-registration")?;
        let registered = other._display.registration_guard().ok_or(ENODEV)?;
        let mut calls = 0;
        check(
            target.with_installed(&registered, |_| {
                calls += 1;
                Ok(())
            }) == Err(EINVAL),
        )?;
        check(calls == 0)
    }

    #[test]
    fn control_of_an_unowned_scene_does_not_authorize_its_pixels() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        let file = fixture.drm.master_file()?;
        let target = target(&fixture, &file)?;
        target.with_current(|current| {
            check(current.output_identity() == fixture.drm.device().output.identity())?;
            check(current.configuration().dimensions() == [640, 480])?;
            check(current.check_scene_owner() == Err(EACCES))
        })?;
        Ok(())
    }

    #[test]
    fn current_control_rechecks_master_loss() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let target = target(&fixture, &file)?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(
            file.file().master_snapshot(),
        ))?;
        fixture.select(&fb, false, 0)?;
        let expected = file.file().master_snapshot().ok_or(EINVAL)?;
        target.with_current(|current| {
            check(current.master() == expected.master())?;
            current.check_scene_owner()
        })?;
        drop(file);
        check(matches!(target.with_current(|_| Ok(())), Err(EACCES)))?;
        Ok(())
    }

    #[test]
    fn retained_control_does_not_keep_an_output_enabled() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let target = target(&fixture, &file)?;
        check(matches!(target.with_current(|_| Ok(())), Err(ENODEV)))?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        target.with_current(|_| Ok(()))?;
        fixture.state.close();
        check(matches!(target.with_current(|_| Ok(())), Err(ENODEV)))?;
        Ok(())
    }

    #[test]
    fn a_target_cannot_substitute_objects_from_another_device() -> Result {
        let fixture = Fixture::new()?;
        let other = Fixture::new_named(c"castkms-control-other")?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let _other_connector = other.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let snapshot = file.file().master_snapshot().ok_or(EINVAL)?;
        let guard = snapshot.master().lock_current().ok_or(EACCES)?;
        check(matches!(
            Target::new(&guard, other.drm.crtc()?, fixture.drm.connector()?),
            Err(EACCES)
        ))?;
        check(matches!(
            Target::new(&guard, fixture.drm.crtc()?, other.drm.connector()?),
            Err(EACCES)
        ))?;
        Ok(())
    }
}
