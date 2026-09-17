// SPDX-License-Identifier: GPL-2.0-only

//! File-issued grants retain permission objects without postponing their creator's close.

use super::*;
use crate::capture::{
    host_stream::Stream,
    permission::Permission,
    provider::Grantor, //
};
use kernel::drm::{
    capture::Status,
    kms::testing::MasterFile, //
};

fn grant(fixture: &Fixture, file: &MasterFile<'_, Driver>) -> Result<Grantor> {
    File::create_capture_grant(file.file(), fixture.drm.crtc()?, fixture.drm.connector()?)
}

fn select(fixture: &Fixture, file: &MasterFile<'_, Driver>) -> Result<FramebufferRef<Driver>> {
    let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(
        file.file().master_snapshot(),
    ))?;
    fixture.select(&fb, false, 0)?;
    Ok(fb)
}

#[kunit_tests(rust_castkms_capture_files)]
mod cases {
    use super::*;

    #[test]
    fn file_grants_capture_active_blank_pixels_without_retaining_old_content() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let fb = select(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let mut stream = Stream::new(&grantor.capture(), 2)?;
        let first = stream.capture()?;
        fixture
            .drm
            .update(|transaction| transaction.disable_plane(fixture.drm.plane()?))?;
        drop(fb);
        let blank = stream.capture()?;
        let mut pixels = KVVec::new();
        pixels.resize(640 * 480 * 4, 0x93, GFP_KERNEL)?;
        check(blank.copy_result(&mut pixels)? == pixels.len())?;
        check(pixels.chunks_exact(4).all(|pixel| pixel == [0, 0, 0, 0xff]))?;
        check(first.status()? == Status::Complete(Ok(())))?;
        Ok(())
    }

    #[test]
    fn a_new_file_cannot_capture_the_previous_masters_unchanged_blank() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = select(&fixture, &file)?;
        fixture
            .drm
            .update(|transaction| transaction.disable_plane(fixture.drm.plane()?))?;
        drop(file);
        let replacement = fixture.drm.master_file()?;
        let grantor = grant(&fixture, &replacement)?;
        fixture.drm.update(|transaction| {
            drop(transaction.add_crtc_state(fixture.drm.crtc()?)?);
            Ok(())
        })?;
        check(matches!(Stream::new(&grantor.capture(), 1), Err(EACCES)))?;
        fixture.drm.update(|transaction| {
            transaction
                .add_crtc_state(fixture.drm.crtc()?)?
                .set_mode_changed(true);
            Ok(())
        })?;
        let mut stream = Stream::new(&grantor.capture(), 1)?;
        check(stream.capture()?.wait()? == Ok(()))?;
        Ok(())
    }

    #[test]
    fn file_grants_precede_display_but_not_pixel_authorization() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let grantor = grant(&fixture, &file)?;
        let capture = grantor.capture();
        check(matches!(capture.stream(1), Err(ENODEV)))?;
        let _fb = select(&fixture, &file)?;
        let mut stream = Stream::new(&capture, 1)?;
        check(stream.capture()?.wait()? == Ok(()))?;
        Ok(())
    }

    #[test]
    fn creator_close_revokes_surviving_grants_without_discarding_completed_results() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = select(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let capture = grantor.capture();
        let mut stream = Stream::new(&capture, 1)?;
        let completed = stream.capture()?;
        drop(file);
        check(matches!(capture.stream(1), Err(EKEYREVOKED)))?;
        check(matches!(stream.capture(), Err(EKEYREVOKED)))?;
        check(completed.status()? == Status::Complete(Ok(())))?;
        let replacement = fixture.drm.master_file()?;
        let _new_fb = select(&fixture, &replacement)?;
        let next = grant(&fixture, &replacement)?;
        check(matches!(capture.stream(1), Err(EKEYREVOKED)))?;
        let mut current = Stream::new(&next.capture(), 1)?;
        check(current.capture()?.wait()? == Ok(()))?;
        drop(grantor);
        check(completed.status()? == Status::Complete(Ok(())))?;
        Ok(())
    }

    #[test]
    fn closing_a_grantor_releases_its_creators_registration() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = select(&fixture, &file)?;
        for _ in 0..128 {
            let grantor = grant(&fixture, &file)?;
            let capture = grantor.capture();
            drop(grantor);
            check(matches!(capture.stream(1), Err(EKEYREVOKED)))?;
        }
        let _fresh = grant(&fixture, &file)?;
        Ok(())
    }

    #[test]
    fn each_creating_file_bounds_its_retained_grantors() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let mut grants = KVec::new();
        for _ in 0..64 {
            grants.push(grant(&fixture, &file)?, GFP_KERNEL)?;
        }
        check(matches!(grant(&fixture, &file), Err(EBUSY)))?;
        drop(grants.pop());
        let replacement = grant(&fixture, &file)?;
        check(matches!(grant(&fixture, &file), Err(EBUSY)))?;
        drop(file);
        for grantor in &grants {
            check(matches!(grantor.capture().stream(1), Err(EKEYREVOKED)))?;
        }
        check(matches!(replacement.capture().stream(1), Err(EKEYREVOKED)))?;
        Ok(())
    }

    #[test]
    fn file_adapter_rejects_foreign_or_unpublished_targets() -> Result {
        let fixture = Fixture::new()?;
        let file = fixture.drm.master_file()?;
        check(matches!(grant(&fixture, &file), Err(EACCES)))?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let foreign = Fixture::new_named(c"castkms-file-foreign")?;
        let _foreign_connector = foreign.drm.publish_connector_identity()?;
        check(matches!(
            File::create_capture_grant(file.file(), foreign.drm.crtc()?, fixture.drm.connector()?),
            Err(EACCES)
        ))?;
        check(matches!(
            File::create_capture_grant(file.file(), fixture.drm.crtc()?, foreign.drm.connector()?),
            Err(EACCES)
        ))?;
        let _grantor = grant(&fixture, &file)?;
        Ok(())
    }

    #[test]
    fn kernel_issuance_does_not_acquire_an_implicit_file_close_owner() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = select(&fixture, &file)?;
        let permission = {
            let snapshot = file.file().master_snapshot().ok_or(EINVAL)?;
            let guard = snapshot.master().lock_current().ok_or(EACCES)?;
            Permission::new(&guard, fixture.drm.crtc()?, fixture.drm.connector()?)?
        };
        let grantor = Grantor::new(permission)?;
        let capture = grantor.capture();
        drop(file);
        // Current access is gone, but no creator registration has revoked the kernel grant.
        check(matches!(capture.stream(1), Err(EACCES)))?;
        drop(grantor);
        check(matches!(capture.stream(1), Err(EKEYREVOKED)))?;
        Ok(())
    }

    #[test]
    fn sharing_the_master_identity_does_not_allow_file_issuance() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let peer = file.associated_file()?;
        let _fb = select(&fixture, &file)?;
        check(peer.file().associated_master() == file.file().associated_master())?;
        check(matches!(
            File::create_capture_grant(peer.file(), fixture.drm.crtc()?, fixture.drm.connector()?),
            Err(EACCES)
        ))?;
        let grantor = grant(&fixture, &file)?;
        let mut stream = Stream::new(&grantor.capture(), 1)?;
        drop(peer);
        check(stream.capture()?.wait()? == Ok(()))?;
        Ok(())
    }

    #[test]
    fn administrative_issuance_uses_a_distinct_current_owner() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let owner_file = fixture.drm.master_file()?;
        let helper_file = owner_file.associated_file()?;
        let _fb = select(&fixture, &owner_file)?;
        check(matches!(
            File::issue_administrative_capture_grant_then_for_test(
                fixture.drm.device(),
                owner_file.file(),
                fixture.drm.crtc()?,
                fixture.drm.connector()?,
                || Ok(()),
            ),
            Err(EAGAIN)
        ))?;
        let grantor = File::issue_administrative_capture_grant_then_for_test(
            fixture.drm.device(),
            helper_file.file(),
            fixture.drm.crtc()?,
            fixture.drm.connector()?,
            || Ok(()),
        )?;
        let mut stream = Stream::new(&grantor.capture(), 1)?;
        check(stream.capture()?.wait()? == Ok(()))
    }

    #[test]
    fn administrative_grant_expires_with_its_owner_interval() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let owner_file = fixture.drm.master_file()?;
        let helper_file = owner_file.associated_file()?;
        let _fb = select(&fixture, &owner_file)?;
        let grantor = File::issue_administrative_capture_grant_then_for_test(
            fixture.drm.device(),
            helper_file.file(),
            fixture.drm.crtc()?,
            fixture.drm.connector()?,
            || Ok(()),
        )?;
        let capture = grantor.capture();
        drop(owner_file);
        check(matches!(capture.stream(1), Err(ESTALE)))?;
        let _replacement = fixture.drm.master_file()?;
        check(matches!(capture.stream(1), Err(ESTALE)))
    }

    #[test]
    fn administrative_issuance_rechecks_the_owner_interval() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let owner_file = fixture.drm.master_file()?;
        let helper_file = owner_file.associated_file()?;
        check(matches!(
            File::issue_administrative_capture_grant_then_for_test(
                fixture.drm.device(),
                helper_file.file(),
                fixture.drm.crtc()?,
                fixture.drm.connector()?,
                || {
                    fixture.drm.device().authority.changed(None);
                    Ok(())
                },
            ),
            Err(ESTALE)
        ))
    }
}
