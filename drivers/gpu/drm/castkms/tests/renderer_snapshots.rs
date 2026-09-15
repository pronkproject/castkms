// SPDX-License-Identifier: GPL-2.0-only

//! Optional startup copies require image ownership without retaining live sources.

use super::*;
use crate::{
    host_compositor::{
        compose,
        layout::Layout,
        pool::Pool, //
    },
    renderer::{
        candidate::Candidate,
        permission::{
            Owner,
            Permission, //
        }, //
    }, //
};
use kernel::{
    drm::kms::testing::MasterFile,
    io::{
        io_project,
        Io, //
    }, //
};

fn owner(fixture: &Fixture, file: &MasterFile<'_, Driver>) -> Result<Owner> {
    let permission = {
        let master = file.file().master_snapshot().ok_or(EINVAL)?;
        let guard = master.master().lock_current().ok_or(EACCES)?;
        Permission::new(&guard, fixture.drm.crtc()?, fixture.drm.connector()?)?
    };
    Owner::new(permission)
}

fn enable(
    fixture: &Fixture,
    file: Option<&MasterFile<'_, Driver>>,
) -> Result<FramebufferRef<Driver>> {
    let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(
        file.and_then(|file| file.file().master_snapshot()),
    ))?;
    fixture.select(&fb, false, 0)?;
    Ok(fb)
}

fn image(fixture: &Fixture) -> Result<compose::Completed> {
    let pool = Pool::new(
        fixture.drm.device(),
        &fixture.host_budget,
        Layout::new(640, 480)?,
    )?;
    compose::current(&fixture.drm.device().output, &pool)?.ok_or(EINVAL)
}

#[kunit_tests(rust_castkms_renderer_snapshots)]
mod cases {
    use super::*;

    #[test]
    fn current_snapshot_reports_absence_without_starting_host_work() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let owner = owner(&fixture, &file)?;
        let _fb = enable(&fixture, Some(&file))?;
        let candidate = Candidate::begin(owner.access())?;
        check(matches!(candidate.snapshot_current(), Err(ENODATA)))?;
        check(matches!(fixture.drm.device().host.current(), Err(EAGAIN)))
    }

    #[test]
    fn current_snapshot_publication_rechecks_the_candidate() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let owner = owner(&fixture, &file)?;
        let fb = enable(&fixture, Some(&file))?;
        {
            let map = fb.vmap::<gem::Object>()?;
            io_project!(map.view(), [try: 0..2560]).copy_from_slice(&[0x35; 2560]);
        }
        fixture.select(&fb, false, 0)?;
        let host = fixture.drm.device().host.configure(
            fixture.drm.device(),
            Layout::new(640, 480)?,
        )?;
        let request = host.request_outcome()?;
        check(matches!(
            request.wait()?,
            crate::host_compositor::worker::Outcome::Image(_)
        ))?;
        let candidate = Candidate::begin(owner.access())?;
        let snapshot = candidate.snapshot_current()?;
        let mut publications = 0;
        candidate.publish_snapshot(&snapshot, || publications += 1)?;
        check(publications == 1)?;
        candidate.cancel();
        check(
            candidate.publish_snapshot(&snapshot, || publications += 1)
                == Err(ECANCELED),
        )?;
        check(publications == 1)?;
        let mut pixels = KVVec::new();
        pixels.resize(snapshot.layout().pixel_bytes(), 0xff, GFP_KERNEL)?;
        snapshot.copy_pixels(&mut pixels)?;
        check(pixels[..2560] == [0x35; 2560])
    }

    #[test]
    fn startup_copy_keeps_earlier_pixels_without_relabeling_content() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let owner = owner(&fixture, &file)?;
        let fb = enable(&fixture, Some(&file))?;
        {
            let map = fb.vmap::<gem::Object>()?;
            io_project!(map.view(), [try: 0..2560]).copy_from_slice(&[0x35; 2560]);
        }
        fixture.select(&fb, false, 0)?;
        let image = image(&fixture)?;
        let serial = image.content_serial();
        let candidate = Candidate::begin(owner.access())?;
        let snapshot =
            candidate.snapshot_then_for_test(&image, || fixture.select(&fb, false, 0))?;
        drop(image);
        check(snapshot.content_serial() == serial)?;
        check(snapshot.configuration() == Some(candidate.configuration()))?;
        check(
            fixture
                .drm
                .device()
                .output
                .inspect(|scene| scene.and_then(scene::Scene::content_serial))
                != serial,
        )?;
        candidate.cancel();
        owner.revoke();
        let mut pixels = KVVec::new();
        pixels.resize(snapshot.layout().pixel_bytes(), 0xff, GFP_KERNEL)?;
        snapshot.copy_pixels(&mut pixels)?;
        check(pixels[..2560] == [0x35; 2560])?;
        check(pixels[2560..].iter().all(|byte| *byte == 0))
    }

    #[test]
    fn control_of_unowned_content_allows_startup_but_not_a_copy() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let _fb = enable(&fixture, None)?;
        check(fixture.has_owner(None))?;
        let file = fixture.drm.master_file()?;
        let owner = owner(&fixture, &file)?;
        let image = image(&fixture)?;
        let candidate = Candidate::begin(owner.access())?;
        check(matches!(candidate.snapshot(&image), Err(EACCES)))?;
        candidate.validate()
    }

    #[test]
    fn adopting_new_content_does_not_authorize_an_unowned_image() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let _unowned = enable(&fixture, None)?;
        let old = image(&fixture)?;
        check(old.owner().is_none())?;
        let file = fixture.drm.master_file()?;
        let owner = owner(&fixture, &file)?;
        let _owned = enable(&fixture, Some(&file))?;
        let candidate = Candidate::begin(owner.access())?;
        check(old.configuration() == Some(candidate.configuration()))?;
        check(matches!(candidate.snapshot(&old), Err(EACCES)))?;
        let current = image(&fixture)?;
        let _snapshot = candidate.snapshot(&current)?;
        Ok(())
    }

    #[test]
    fn revocation_during_copy_discards_storage_without_canceling_a_new_issuer() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = enable(&fixture, Some(&file))?;
        let image = image(&fixture)?;
        for _ in 0..20 {
            let owner = owner(&fixture, &file)?;
            let candidate = Candidate::begin(owner.access())?;
            let result = candidate.snapshot_then_for_test(&image, || {
                owner.revoke();
                Ok(())
            });
            check(matches!(result, Err(EKEYREVOKED)))?;
        }
        let owner = owner(&fixture, &file)?;
        let candidate = Candidate::begin(owner.access())?;
        let _snapshot = candidate.snapshot(&image)?;
        Ok(())
    }

    #[test]
    fn cancellation_during_copy_does_not_cancel_a_replacement() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let owner = owner(&fixture, &file)?;
        let _fb = enable(&fixture, Some(&file))?;
        let image = image(&fixture)?;
        let candidate = Candidate::begin(owner.access())?;
        let mut replacement = None;
        let result = candidate.snapshot_then_for_test(&image, || {
            candidate.cancel();
            replacement = Some(Candidate::begin(owner.access())?);
            Ok(())
        });
        check(matches!(result, Err(ECANCELED)))?;
        drop(candidate);
        let replacement = replacement.ok_or(EINVAL)?;
        let _snapshot = replacement.snapshot(&image)?;
        replacement.validate()
    }

    #[test]
    fn a_changed_display_interval_rejects_the_copy() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let owner = owner(&fixture, &file)?;
        let fb = enable(&fixture, Some(&file))?;
        let image = image(&fixture)?;
        let candidate = Candidate::begin(owner.access())?;
        let result = candidate.snapshot_then_for_test(&image, || {
            fixture
                .drm
                .update(|mut state| state.as_mut().set_crtc_config(fixture.drm.crtc()?, None))?;
            fixture.select(&fb, false, 0)
        });
        check(matches!(result, Err(ESTALE)))?;
        drop(candidate);
        let replacement = Candidate::begin(owner.access())?;
        check(matches!(replacement.snapshot(&image), Err(EACCES)))?;
        let current = super::image(&fixture)?;
        let _snapshot = replacement.snapshot(&current)?;
        Ok(())
    }

    #[test]
    fn losing_native_master_control_during_copy_rejects_the_result() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let owner = owner(&fixture, &file)?;
        let _fb = enable(&fixture, Some(&file))?;
        let image = image(&fixture)?;
        let candidate = Candidate::begin(owner.access())?;
        let result = candidate.snapshot_then_for_test(&image, || {
            drop(file);
            Ok(())
        });
        check(matches!(result, Err(EACCES)))?;
        check(candidate.validate() == Err(EACCES))
    }

    #[test]
    fn private_copy_failure_returns_its_storage_credit() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let owner = owner(&fixture, &file)?;
        let _fb = enable(&fixture, Some(&file))?;
        let image = image(&fixture)?;
        let candidate = Candidate::begin(owner.access())?;
        for _ in 0..20 {
            check(matches!(
                candidate.snapshot_then_for_test(&image, || Err(EIO)),
                Err(EIO)
            ))?;
        }
        let _snapshot = candidate.snapshot(&image)?;
        candidate.validate()
    }

    #[test]
    fn closing_the_output_during_copy_rejects_the_private_result() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let owner = owner(&fixture, &file)?;
        let _fb = enable(&fixture, Some(&file))?;
        let image = image(&fixture)?;
        let candidate = Candidate::begin(owner.access())?;
        let result = candidate.snapshot_then_for_test(&image, || {
            fixture.state.close();
            Ok(())
        });
        check(matches!(result, Err(ENODEV)))?;
        check(candidate.validate() == Err(ENODEV))?;
        let mut row = KVVec::new();
        row.resize(2560, 0xff, GFP_KERNEL)?;
        image.read_row(0, &mut row)?;
        check(row.iter().all(|byte| *byte == 0))
    }

    #[test]
    fn a_foreign_output_is_rejected_even_with_matching_owner_and_configuration() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let owner = owner(&fixture, &file)?;
        let _fb = enable(&fixture, Some(&file))?;
        let candidate = Candidate::begin(owner.access())?;
        let other = kernel::sync::Arc::pin_init(Output::new(), GFP_KERNEL)?;
        other.publish_with_configuration(
            kernel::drm::preparation::Source::new(1)?,
            output::SceneUpdate::Replace(
                fixture.drm.device().output.inspect(|scene| scene.cloned()),
            ),
            Some(candidate.configuration().clone()),
        );
        let result = (|| {
            let pool = Pool::new(
                fixture.drm.device(),
                &fixture.host_budget,
                Layout::new(640, 480)?,
            )?;
            let image = compose::current(&other, &pool)?.ok_or(EINVAL)?;
            check(image.configuration() == Some(candidate.configuration()))?;
            check(matches!(candidate.snapshot(&image), Err(EACCES)))
        })();
        other.close();
        result
    }

    #[test]
    fn retained_copies_share_a_budget_across_authorized_candidates() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let owner = owner(&fixture, &file)?;
        let _fb = enable(&fixture, Some(&file))?;
        let image = image(&fixture)?;
        let capacity = (16usize * 1024 * 1024)
            .checked_div(image.layout().size())
            .ok_or(EINVAL)?;
        let mut copies = KVec::new();
        for _ in 0..capacity {
            let candidate = Candidate::begin(owner.access())?;
            copies.push(candidate.snapshot(&image)?, GFP_KERNEL)?;
        }
        let candidate = Candidate::begin(owner.access())?;
        check(matches!(candidate.snapshot(&image), Err(EBUSY)))?;
        candidate.validate()?;
        drop(image);
        let current = super::image(&fixture)?;
        check(matches!(candidate.snapshot(&current), Err(EBUSY)))?;
        drop(copies.pop());
        let _replacement = candidate.snapshot(&current)?;
        Ok(())
    }
}
