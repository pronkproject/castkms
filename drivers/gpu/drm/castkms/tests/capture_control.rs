// SPDX-License-Identifier: GPL-2.0-only

//! Transferring complete grantors to revocation-only files preserves both close paths.

use super::*;
use crate::capture::{
    host_stream::Stream,
    permission::Permission,
    provider::Grantor, //
};
use kernel::{
    drm::{
        capture::Status,
        kms::testing::MasterFile, //
    },
    fs::File as ControlEndpoint,
    sync::aref::ARef, //
};

/// Finishes final file cleanup before a test releases its faux device, including error exits.
#[derive(Clone)]
struct ControlFile(Option<ARef<ControlEndpoint>>);

impl ControlFile {
    fn new(grantor: Grantor) -> Result<Self> {
        Ok(Self(Some(grantor.into_control_file()?)))
    }
}

impl Drop for ControlFile {
    fn drop(&mut self) {
        if let Some(file) = self.0.take() {
            // SAFETY: KUnit runs in a kernel thread. Transfer the owned reference to final
            // fput without deferring policy destruction past the test device's lifetime.
            unsafe { kernel::bindings::__fput_sync(ARef::into_raw(file).cast().as_ptr()) };
        }
    }
}

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

#[kunit_tests(rust_castkms_capture_control)]
mod cases {
    use super::*;

    #[test]
    fn transferred_grant_revokes_only_after_the_last_control_reference() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let creator = fixture.drm.master_file()?;
        let _fb = select(&fixture, &creator)?;
        let grantor = grant(&fixture, &creator)?;
        let capture = grantor.capture();
        let control = ControlFile::new(grantor)?;
        let duplicate = control.clone();
        let mut stream = Stream::new(&capture, 2)?;
        let completed = stream.capture()?;
        drop(control);
        check(stream.capture()?.wait()? == Ok(()))?;
        drop(duplicate);
        check(matches!(capture.stream(1), Err(EKEYREVOKED)))?;
        check(matches!(stream.capture(), Err(EKEYREVOKED)))?;
        check(completed.status()? == Status::Complete(Ok(())))?;
        let mut bytes = KVVec::new();
        bytes.resize(640 * 480 * 4, 0, GFP_KERNEL)?;
        check(completed.copy_result(&mut bytes)? == bytes.len())?;
        Ok(())
    }

    #[test]
    fn creator_close_revokes_a_transferred_grant_with_live_control_files() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let creator = fixture.drm.master_file()?;
        let _fb = select(&fixture, &creator)?;
        let grantor = grant(&fixture, &creator)?;
        let capture = grantor.capture();
        let control = ControlFile::new(grantor)?;
        let duplicate = control.clone();
        let mut stream = Stream::new(&capture, 1)?;
        let completed = stream.capture()?;
        drop(creator);
        check(matches!(capture.stream(1), Err(EKEYREVOKED)))?;
        check(matches!(stream.capture(), Err(EKEYREVOKED)))?;
        drop(control);
        drop(duplicate);
        check(completed.status()? == Status::Complete(Ok(())))?;
        Ok(())
    }

    #[test]
    fn final_control_close_returns_one_creator_registration() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let creator = fixture.drm.master_file()?;
        let mut controls = KVec::new();
        for _ in 0..64 {
            controls.push(ControlFile::new(grant(&fixture, &creator)?)?, GFP_KERNEL)?;
        }
        check(matches!(grant(&fixture, &creator), Err(EBUSY)))?;
        let control = controls.pop().ok_or(EINVAL)?;
        let duplicate = control.clone();
        drop(control);
        check(matches!(grant(&fixture, &creator), Err(EBUSY)))?;
        drop(duplicate);
        let replacement = ControlFile::new(grant(&fixture, &creator)?)?;
        check(matches!(grant(&fixture, &creator), Err(EBUSY)))?;
        drop(replacement);
        let _next = grant(&fixture, &creator)?;
        Ok(())
    }

    #[test]
    fn independent_controls_do_not_share_revocation_ownership() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let creator = fixture.drm.master_file()?;
        let _fb = select(&fixture, &creator)?;
        let first = grant(&fixture, &creator)?;
        let first_capture = first.capture();
        let first_control = ControlFile::new(first)?;
        let second = grant(&fixture, &creator)?;
        let second_capture = second.capture();
        let _second_control = ControlFile::new(second)?;
        drop(first_control);
        check(matches!(first_capture.stream(1), Err(EKEYREVOKED)))?;
        let mut stream = Stream::new(&second_capture, 1)?;
        check(stream.capture()?.wait()? == Ok(()))?;
        Ok(())
    }

    #[test]
    fn kernel_grant_transfer_does_not_invent_a_creator_close_dependency() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let creator = fixture.drm.master_file()?;
        let _fb = select(&fixture, &creator)?;
        let permission = {
            let snapshot = creator.file().master_snapshot().ok_or(EINVAL)?;
            let current = snapshot.master().lock_current().ok_or(EACCES)?;
            Permission::new(&current, fixture.drm.crtc()?, fixture.drm.connector()?)?
        };
        let grantor = Grantor::new(permission)?;
        let capture = grantor.capture();
        let control = ControlFile::new(grantor)?;
        drop(creator);
        check(matches!(capture.stream(1), Err(EACCES)))?;
        drop(control);
        check(matches!(capture.stream(1), Err(EKEYREVOKED)))?;
        Ok(())
    }

    #[test]
    fn transferring_after_creator_close_does_not_reopen_the_grant() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let creator = fixture.drm.master_file()?;
        let grantor = grant(&fixture, &creator)?;
        let capture = grantor.capture();
        drop(creator);
        let _control = ControlFile::new(grantor)?;
        check(matches!(capture.stream(1), Err(EKEYREVOKED)))?;
        Ok(())
    }
}
