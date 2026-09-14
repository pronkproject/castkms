// SPDX-License-Identifier: GPL-2.0-only

//! Capture-client files never inherit the separate grantor or creating DRM file lifetime.

use super::*;
use crate::capture::provider::Capture;
use kernel::fs::File as ClientEndpoint;

#[derive(Clone)]
struct ClientFile(Option<ARef<ClientEndpoint>>);

impl ClientFile {
    fn new(capture: Capture) -> Result<Self> {
        Ok(Self(Some(capture.into_client_file()?)))
    }

    fn is_revoked(&self) -> Result<bool> {
        let file = self.0.as_ref().ok_or(EINVAL)?;
        // SAFETY: The owned anonymous file retains its immutable operations and module.
        let poll = unsafe { (*(*file.as_ptr()).f_op).poll }.ok_or(EINVAL)?;
        // SAFETY: The file remains live; a null table observes readiness without retaining a waiter.
        let events = unsafe { poll(file.as_ptr(), core::ptr::null_mut()) };
        Ok(events & kernel::bindings::POLLHUP != 0)
    }
}

impl Drop for ClientFile {
    fn drop(&mut self) {
        if let Some(file) = self.0.take() {
            // SAFETY: KUnit is a kernel thread. Finish the owned reference's release before
            // the test's faux device is destroyed, including when assertions return early.
            unsafe { kernel::bindings::__fput_sync(ARef::into_raw(file).cast().as_ptr()) };
        }
    }
}

#[kunit_tests(rust_castkms_capture_client)]
mod cases {
    use super::*;

    #[test]
    fn client_close_does_not_revoke_siblings_or_retain_the_grantor() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let creator = fixture.drm.master_file()?;
        let _fb = select(&fixture, &creator)?;
        let grantor = grant(&fixture, &creator)?;
        let capture = grantor.capture();
        let first = ClientFile::new(capture.clone())?;
        let second = ClientFile::new(capture.clone())?;
        drop(first.clone());
        drop(first);
        check(!second.is_revoked()?)?;
        let mut stream = Stream::new(&capture, 1)?;
        check(stream.capture()?.status()? == Status::Complete(Ok(())))?;
        drop(grantor);
        check(second.is_revoked()?)?;
        check(matches!(Stream::new(&capture, 1), Err(EKEYREVOKED)))
    }

    #[test]
    fn retained_client_does_not_postpone_creator_close() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let creator = fixture.drm.master_file()?;
        let _fb = select(&fixture, &creator)?;
        let grantor = grant(&fixture, &creator)?;
        let capture = grantor.capture();
        let client = ClientFile::new(capture.clone())?;
        check(!client.is_revoked()?)?;
        drop(creator);
        check(client.is_revoked()?)?;
        check(matches!(Stream::new(&capture, 1), Err(EKEYREVOKED)))
    }

    #[test]
    fn shutdown_revokes_clients_without_pixel_operations() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let creator = fixture.drm.master_file()?;
        let grantor = grant(&fixture, &creator)?;
        let capture = grantor.capture();
        let client = ClientFile::new(capture.clone())?;
        fixture.state.close();
        check(client.is_revoked()?)?;
        check(matches!(capture.stream(1), Err(EKEYREVOKED)))
    }
}
