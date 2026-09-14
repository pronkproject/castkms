// SPDX-License-Identifier: GPL-2.0-only

//! Candidate startup follows control and configuration, not changing pixel content.

use super::*;
use crate::renderer::{
    candidate::Candidate,
    permission::{
        Owner,
        Permission, //
    }, //
};
use kernel::drm::kms::testing::MasterFile;

fn owner(fixture: &Fixture, file: &MasterFile<'_, Driver>) -> Result<Owner> {
    let permission = {
        let master = file.file().master_snapshot().ok_or(EINVAL)?;
        let guard = master.master().lock_current().ok_or(EACCES)?;
        Permission::new(&guard, fixture.drm.crtc()?, fixture.drm.connector()?)?
    };
    Owner::new(permission)
}

fn enable(fixture: &Fixture) -> Result<FramebufferRef<Driver>> {
    let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
    fixture.select(&fb, false, 0)?;
    Ok(fb)
}

#[kunit_tests(rust_castkms_renderer_candidates)]
mod cases {
    use super::*;

    #[test]
    fn activation_control_rejects_a_foreign_registration_without_consuming_startup() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let _fb = enable(&fixture)?;
        let file = fixture.drm.master_file()?;
        let owner = owner(&fixture, &file)?;
        let candidate = Candidate::begin(owner.access())?;
        let other = CastKms::new(c"castkms-candidate-registration")?;
        let registered = other._display.registration_guard().ok_or(ENODEV)?;
        let mut calls = 0;
        check(
            candidate.with_activation_control(&registered, |_| {
                calls += 1;
                Ok(())
            }) == Err(EINVAL),
        )?;
        check(calls == 0)?;
        candidate.validate()
    }

    #[test]
    fn private_startup_survives_continuously_changing_unowned_content() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let fb = enable(&fixture)?;
        check(fixture.has_owner(None))?;
        let file = fixture.drm.master_file()?;
        let owner = owner(&fixture, &file)?;
        let candidate = Candidate::begin(owner.access())?;
        let first_content = fixture
            .drm
            .device()
            .output
            .inspect(|scene| scene.map(scene::Scene::content_serial));
        for _ in 0..32 {
            fixture.select(&fb, false, 0)?;
            candidate.validate()?;
            check(fixture.has_owner(None))?;
        }
        check(candidate.configuration().dimensions() == [640, 480])?;
        check(
            fixture
                .drm
                .device()
                .output
                .inspect(|scene| scene.map(scene::Scene::content_serial))
                != first_content,
        )?;
        check(
            crate::execution::describe()
                == crate::execution::Description {
                    profile: crate::execution::Profile::HostV1,
                    generation: 1,
                },
        )
    }

    #[test]
    fn disable_and_reenable_require_a_new_candidate() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let owner = owner(&fixture, &file)?;
        let fb = enable(&fixture)?;
        let candidate = Candidate::begin(owner.access())?;
        fixture
            .drm
            .update(|mut state| state.as_mut().set_crtc_config(fixture.drm.crtc()?, None))?;
        check(candidate.validate() == Err(ENODEV))?;
        fixture.select(&fb, false, 0)?;
        check(candidate.validate() == Err(ESTALE))?;
        drop(candidate);
        let replacement = Candidate::begin(owner.access())?;
        replacement.validate()
    }

    #[test]
    fn revoked_access_cannot_reserve_or_validate_startup() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let owner = owner(&fixture, &file)?;
        let _fb = enable(&fixture)?;
        let candidate = Candidate::begin(owner.access())?;
        owner.revoke();
        check(candidate.validate() == Err(EKEYREVOKED))?;
        drop(candidate);
        check(matches!(Candidate::begin(owner.access()), Err(EKEYREVOKED)))?;
        let fresh = super::owner(&fixture, &file)?;
        let replacement = Candidate::begin(fresh.access())?;
        replacement.validate()
    }

    #[test]
    fn canceled_objects_cannot_validate_or_cancel_replacement() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let owner = owner(&fixture, &file)?;
        let _fb = enable(&fixture)?;
        let first = Candidate::begin(owner.access())?;
        check(matches!(Candidate::begin(owner.access()), Err(EBUSY)))?;
        first.cancel();
        check(first.validate() == Err(ECANCELED))?;
        let second = Candidate::begin(owner.access())?;
        first.cancel();
        drop(first);
        second.validate()
    }

    #[test]
    fn a_disabled_output_does_not_consume_the_startup_slot() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let owner = owner(&fixture, &file)?;
        check(matches!(Candidate::begin(owner.access()), Err(ENODEV)))?;
        let _fb = enable(&fixture)?;
        let candidate = Candidate::begin(owner.access())?;
        candidate.validate()
    }

    #[test]
    fn revocation_during_reservation_returns_the_slot() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let owner = owner(&fixture, &file)?;
        let _fb = enable(&fixture)?;
        let result = Candidate::begin_then_for_test(owner.access(), || {
            owner.revoke();
            Ok(())
        });
        check(matches!(result, Err(EKEYREVOKED)))?;
        let replacement_owner = super::owner(&fixture, &file)?;
        let replacement = Candidate::begin(replacement_owner.access())?;
        replacement.validate()
    }

    #[test]
    fn changed_configuration_during_reservation_returns_the_slot() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let owner = owner(&fixture, &file)?;
        let fb = enable(&fixture)?;
        let result = Candidate::begin_then_for_test(owner.access(), || {
            fixture
                .drm
                .update(|mut state| state.as_mut().set_crtc_config(fixture.drm.crtc()?, None))?;
            fixture.select(&fb, false, 0)
        });
        check(matches!(result, Err(ESTALE)))?;
        let replacement = Candidate::begin(owner.access())?;
        replacement.validate()
    }

    #[test]
    fn content_changed_during_reservation_does_not_restart_startup() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let owner = owner(&fixture, &file)?;
        let fb = enable(&fixture)?;
        let candidate =
            Candidate::begin_then_for_test(owner.access(), || fixture.select(&fb, false, 0))?;
        candidate.validate()
    }
}
