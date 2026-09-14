// SPDX-License-Identifier: GPL-2.0-only

//! Capture-client files never inherit the separate grantor or creating DRM file lifetime.

mod description;
mod destinations;
mod stream_files;
mod streams;

use super::*;
use crate::capture::provider::Capture;
use kernel::drm::{
    capture::Target,
    kms::{
        connector::AsRawConnector,
        crtc::AsRawCrtc,
        testing::RegisteredMasterFile, //
    }, //
};
use kernel::fs::File as ClientEndpoint;

fn registered_target(file: &RegisteredMasterFile<'_, Driver>) -> Result<Target> {
    let crtc = file.crtc()?;
    let connector = file.connector()?;
    // SAFETY: The registered file and returned references retain initialized mode objects.
    unsafe { Target::new((*crtc.as_raw()).base.id, (*connector.as_raw()).base.id) }
}

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
    fn failed_client_construction_does_not_take_revocation_ownership() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let creator = fixture.drm.master_file()?;
        let _fb = select(&fixture, &creator)?;
        let grantor = grant(&fixture, &creator)?;
        let capture = grantor.capture();
        check(matches!(
            capture.clone().into_client_file_with::<Capture>(|_| Err(ENOMEM)),
            Err(ENOMEM)
        ))?;
        let description = capture.describe_stream()?;
        check(description.layout().dimensions() == (640, 480))?;
        let client = ClientFile(Some(capture.into_client_file_with(|capture| {
            check(capture.describe_stream()?.layout() == description.layout())?;
            Ok(capture)
        })?));
        check(!client.is_revoked()?)?;
        drop(grantor);
        check(client.is_revoked()?)
    }

    #[test]
    fn failed_pair_construction_releases_the_consumed_grantor() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let creator = fixture.drm.master_file()?;
        let _fb = select(&fixture, &creator)?;
        let grantor = grant(&fixture, &creator)?;
        let capture = grantor.capture();
        check(matches!(
            grantor.into_files_with::<Capture>(|_| Err(ENOMEM)),
            Err(ENOMEM)
        ))?;
        check(matches!(capture.describe_stream(), Err(EKEYREVOKED)))?;
        let next = grant(&fixture, &creator)?;
        let (client, control) = next.into_files_with(Ok)?.into_files();
        let client = ClientFile(Some(client));
        let control = ControlFile(Some(control));
        check(!client.is_revoked()?)?;
        drop(control);
        check(client.is_revoked()?)
    }

    #[test]
    fn generic_issuance_keeps_creator_close_separate_from_client_close() -> Result {
        let display = CastKms::new(c"castkms-file-grant")?;
        let dev = display._display.registration_guard().ok_or(ENODEV)?;
        let creator = RegisteredMasterFile::new(&dev)?;
        let target = registered_target(&creator)?;
        let (client, control) = dev
            .create_capture_grant(creator.file(), target)?
            .into_files();
        let client = ClientFile(Some(client));
        let control = ControlFile(Some(control));
        check(!client.is_revoked()?)?;
        drop(client);
        check(!control.is_revoked()?)?;
        drop(creator);
        check(control.is_revoked()?)
    }

    #[test]
    fn generic_issuance_rejects_foreign_files_and_wrong_object_types() -> Result {
        let display = CastKms::new(c"castkms-grant-target")?;
        let foreign = CastKms::new(c"castkms-grant-foreign")?;
        let dev = display._display.registration_guard().ok_or(ENODEV)?;
        let other = foreign._display.registration_guard().ok_or(ENODEV)?;
        let creator = RegisteredMasterFile::new(&dev)?;
        let target = registered_target(&creator)?;
        check(matches!(
            other.create_capture_grant(creator.file(), target),
            Err(EINVAL)
        ))?;
        let swapped = Target::new(target.connector_id(), target.crtc_id())?;
        check(matches!(
            dev.create_capture_grant(creator.file(), swapped),
            Err(ENOENT)
        ))?;
        let (client, control) = dev
            .create_capture_grant(creator.file(), target)?
            .into_files();
        let client = ClientFile(Some(client));
        let control = ControlFile(Some(control));
        drop(creator);
        check(client.is_revoked()?)?;
        check(control.is_revoked()?)
    }

    #[test]
    fn assembled_pair_keeps_revocation_in_the_control_endpoint() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let creator = fixture.drm.master_file()?;
        let _fb = select(&fixture, &creator)?;
        let grantor = grant(&fixture, &creator)?;
        let capture = grantor.capture();
        let (client, control) = grantor.into_files()?.into_files();
        let client = ClientFile(Some(client));
        let control = ControlFile(Some(control));
        drop(client);
        check(!control.is_revoked()?)?;
        let mut stream = Stream::new(&capture, 1)?;
        check(stream.capture()?.status()? == Status::Complete(Ok(())))?;
        drop(control);
        check(matches!(Stream::new(&capture, 1), Err(EKEYREVOKED)))
    }

    #[test]
    fn assembled_pair_preserves_the_creators_revocation_edge() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let creator = fixture.drm.master_file()?;
        let grantor = grant(&fixture, &creator)?;
        let (client, control) = grantor.into_files()?.into_files();
        let client = ClientFile(Some(client));
        let control = ControlFile(Some(control));
        drop(creator);
        check(client.is_revoked()?)?;
        check(control.is_revoked()?)
    }

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
