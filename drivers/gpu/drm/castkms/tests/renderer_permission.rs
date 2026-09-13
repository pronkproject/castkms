// SPDX-License-Identifier: GPL-2.0-only

//! Renderer control has its own issuer and never implies image ownership.

use super::*;
use crate::renderer::permission::{
    Owner,
    Permission, //
};
use kernel::drm::kms::testing::MasterFile;

fn owner(fixture: &Fixture, file: &MasterFile<'_, Driver>) -> Result<Owner> {
    let permission = {
        let snapshot = file.file().master_snapshot().ok_or(EINVAL)?;
        let guard = snapshot.master().lock_current().ok_or(EACCES)?;
        Permission::new(&guard, fixture.drm.crtc()?, fixture.drm.connector()?)?
    };
    Owner::new(permission)
}

fn enable(fixture: &Fixture) -> Result {
    let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
    fixture.select(&fb, false, 0)
}

fn capture_grant(
    fixture: &Fixture,
    file: &MasterFile<'_, Driver>,
) -> Result<crate::capture::provider::Grantor> {
    let permission = {
        let snapshot = file.file().master_snapshot().ok_or(EINVAL)?;
        let guard = snapshot.master().lock_current().ok_or(EACCES)?;
        crate::capture::permission::Permission::new(
            &guard,
            fixture.drm.crtc()?,
            fixture.drm.connector()?,
        )?
    };
    crate::capture::provider::Grantor::new(permission)
}

#[kunit_tests(rust_castkms_renderer_permission)]
mod cases {
    use super::*;

    #[test]
    fn startup_control_does_not_require_or_grant_image_ownership() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        enable(&fixture)?;
        let file = fixture.drm.master_file()?;
        let owner = owner(&fixture, &file)?;
        let access = owner.access();
        check(core::ptr::eq(access.device(), fixture.drm.device()))?;
        access.with_current(|current| {
            check(current.configuration().dimensions() == [640, 480])?;
            check(current.check_scene_owner() == Err(EACCES))
        })?;
        let capture = capture_grant(&fixture, &file)?;
        check(matches!(capture.capture().stream(1), Err(EACCES)))
    }

    #[test]
    fn revocation_stops_all_retained_handles() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        enable(&fixture)?;
        let owner = owner(&fixture, &file)?;
        let access = owner.access();
        let clone = access.clone();
        owner.revoke();
        check(matches!(access.with_current(|_| Ok(())), Err(EKEYREVOKED)))?;
        check(matches!(clone.with_current(|_| Ok(())), Err(EKEYREVOKED)))?;
        drop(owner);
        check(matches!(clone.with_current(|_| Ok(())), Err(EKEYREVOKED)))
    }

    #[test]
    fn dropping_the_issuer_revokes_access_without_waiting_for_handles() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        enable(&fixture)?;
        let owner = owner(&fixture, &file)?;
        let access = owner.access();
        drop(owner);
        check(matches!(access.with_current(|_| Ok(())), Err(EKEYREVOKED)))
    }

    #[test]
    fn a_new_control_interval_does_not_revive_an_old_permission() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        enable(&fixture)?;
        let owner = owner(&fixture, &file)?;
        let access = owner.access();
        let master = file.file().master_snapshot().ok_or(EINVAL)?;
        // Model ordered callbacks while keeping the native identity available for lookup.
        fixture.drm.device().authority.changed(None);
        fixture
            .drm
            .device()
            .authority
            .changed(Some(master.master().clone()));
        check(matches!(access.with_current(|_| Ok(())), Err(ESTALE)))?;
        let replacement = super::owner(&fixture, &file)?;
        replacement.access().with_current(|_| Ok(()))
    }

    #[test]
    fn renderer_handles_do_not_extend_native_master_control() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        enable(&fixture)?;
        let owner = owner(&fixture, &file)?;
        let access = owner.access();
        drop(file);
        check(matches!(access.with_current(|_| Ok(())), Err(EACCES)))
    }

    #[test]
    fn closing_the_device_rejects_retained_renderer_access() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        enable(&fixture)?;
        let owner = owner(&fixture, &file)?;
        let access = owner.access();
        fixture.state.close();
        check(matches!(access.with_current(|_| Ok(())), Err(ENODEV)))
    }

    #[test]
    fn revoking_one_issuer_does_not_revoke_another() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        enable(&fixture)?;
        let first = owner(&fixture, &file)?;
        let second = owner(&fixture, &file)?;
        first.revoke();
        second.access().with_current(|_| Ok(()))
    }

    #[test]
    fn renderer_and_capture_issuers_have_independent_lifetimes() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(
            file.file().master_snapshot(),
        ))?;
        fixture.select(&fb, false, 0)?;
        let owner = owner(&fixture, &file)?;
        let access = owner.access();
        let grantor = capture_grant(&fixture, &file)?;
        let capture = grantor.capture();
        drop(grantor);
        check(matches!(capture.stream(1), Err(EKEYREVOKED)))?;
        access.with_current(|_| Ok(()))?;
        let replacement = capture_grant(&fixture, &file)?;
        owner.revoke();
        check(matches!(access.with_current(|_| Ok(())), Err(EKEYREVOKED)))?;
        let stream = replacement.capture().stream(1)?;
        check(stream.queue()?.status()? == kernel::drm::capture::Status::Pending)
    }
}
