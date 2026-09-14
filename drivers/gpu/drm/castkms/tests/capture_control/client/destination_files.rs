// SPDX-License-Identifier: GPL-2.0-only

//! Shared file callbacks retain CastKMS output storage without adding file-only policy.

use super::*;
use crate::capture::client::Client;
use kernel::drm::{
    capture::{
        ClientDestination,
        Destination,
        DestinationPlane, //
    },
    fourcc, //
};

#[kunit_tests(rust_castkms_client_destination_files)]
mod cases {
    use super::*;

    #[test]
    fn registration_and_revoked_cleanup_use_the_kernel_client_policy() -> Result {
        with_exporter(|fixture| {
            let _connector = fixture.drm.publish_connector_identity()?;
            let creator = fixture.drm.master_file()?;
            let _fb = select(fixture, &creator)?;
            let grantor = grant(fixture, &creator)?;
            let client = ClientFile(Some(grantor.capture().into_client_file_with(Client::new)?));
            let file = client.0.as_ref().ok_or(EINVAL)?;
            let buffer = fixture.drm.export_dumb(640, 480, 32)?;
            let destination = Destination::new(
                [640, 480],
                fourcc::XRGB8888,
                0,
                &[DestinationPlane::new(&buffer, 2560, 0)],
            )?;
            let mut registered = ClientDestination::register(file, 1, &destination)?;
            check(registered.id() == Some(1))?;
            drop(grantor);
            check(matches!(
                ClientDestination::register(file, 2, &destination),
                Err(EKEYREVOKED)
            ))?;
            registered.unregister()?;
            check(registered.id().is_none())?;
            registered.unregister()
        })
    }

    #[test]
    fn provider_layout_rejections_do_not_consume_the_requested_name() -> Result {
        with_exporter(|fixture| {
            let _connector = fixture.drm.publish_connector_identity()?;
            let creator = fixture.drm.master_file()?;
            let _fb = select(fixture, &creator)?;
            let grantor = grant(fixture, &creator)?;
            let client = ClientFile(Some(grantor.capture().into_client_file_with(Client::new)?));
            let file = client.0.as_ref().ok_or(EINVAL)?;
            let buffer = fixture.drm.export_dumb(640, 480, 32)?;
            let plane = DestinationPlane::new(&buffer, 2560, 0);
            let multiple = Destination::new([640, 480], fourcc::XRGB8888, 0, &[plane; 2])?;
            check(matches!(
                ClientDestination::register(file, 1, &multiple),
                Err(EOPNOTSUPP)
            ))?;
            let format = Destination::new([640, 480], fourcc::ARGB8888, 0, &[plane])?;
            check(matches!(
                ClientDestination::register(file, 1, &format),
                Err(EOPNOTSUPP)
            ))?;
            let offset = Destination::new(
                [640, 480],
                fourcc::XRGB8888,
                0,
                &[DestinationPlane::new(&buffer, 2560, 4)],
            )?;
            check(matches!(
                ClientDestination::register(file, 1, &offset),
                Err(EINVAL)
            ))?;
            let valid = Destination::new([640, 480], fourcc::XRGB8888, 0, &[plane])?;
            let registered = ClientDestination::register(file, 1, &valid)?;
            check(registered.id() == Some(1))
        })
    }

    #[test]
    fn exhausted_destination_names_preserve_removal() -> Result {
        with_exporter(|fixture| {
            let _connector = fixture.drm.publish_connector_identity()?;
            let creator = fixture.drm.master_file()?;
            let _fb = select(fixture, &creator)?;
            let grantor = grant(fixture, &creator)?;
            let client = ClientFile(Some(grantor.capture().into_client_file_with(Client::new)?));
            let file = client.0.as_ref().ok_or(EINVAL)?;
            let buffer = fixture.drm.export_dumb(640, 480, 32)?;
            let destination = Destination::new(
                [640, 480],
                fourcc::XRGB8888,
                0,
                &[DestinationPlane::new(&buffer, 2560, 0)],
            )?;
            let mut registered = ClientDestination::register(file, u64::MAX, &destination)?;
            check(matches!(
                ClientDestination::register(file, 1, &destination),
                Err(EOVERFLOW)
            ))?;
            registered.unregister()?;
            check(matches!(
                ClientDestination::register(file, 1, &destination),
                Err(EOVERFLOW)
            ))
        })
    }
}
