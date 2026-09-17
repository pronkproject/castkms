// SPDX-License-Identifier: GPL-2.0-only

//! Output registrations keep client names and retained allocation lifetimes separate.

use super::*;
use crate::{
    capture::{
        client::Client,
        destination::Image, //
    },
    host_compositor::layout::Layout, //
};
use kernel::drm::{
    capture::ClientOwner,
    fourcc, //
};

fn image(fixture: &Fixture) -> Result<Image> {
    Image::new(
        fixture.drm.export_dumb(640, 480, 32)?,
        Layout::new(640, 480)?,
        fourcc::XRGB8888,
        fourcc::FORMAT_MOD_LINEAR,
        2560,
        0,
    )
}

#[kunit_tests(rust_castkms_client_destinations)]
mod cases {
    use super::*;

    #[test]
    fn known_alias_rejection_does_not_consume_a_destination_name() -> Result {
        with_exporter(|fixture| {
            let _connector = fixture.drm.publish_connector_identity()?;
            let creator = fixture.drm.master_file()?;
            let _fb = select(fixture, &creator)?;
            let grantor = grant(fixture, &creator)?;
            let mut client = Client::new(grantor.capture())?;
            client.register_destination(1, image(fixture)?)?;
            let retained = client.destination(1)?;
            let alias = Image::new(
                retained.buffer().into(), Layout::new(640, 480)?, fourcc::XRGB8888,
                fourcc::FORMAT_MOD_LINEAR, retained.pitch(), retained.offset(),
            )?;
            check(client.register_destination(2, alias) == Err(EEXIST))?;
            client.register_destination(2, image(fixture)?)?;
            client.unregister_destination(1)?;
            client.unregister_destination(2)
        })
    }

    #[test]
    fn names_are_independent_and_removed_storage_survives_only_by_reference() -> Result {
        with_exporter(|fixture| {
            let _connector = fixture.drm.publish_connector_identity()?;
            let creator = fixture.drm.master_file()?;
            let _fb = select(fixture, &creator)?;
            let grantor = grant(fixture, &creator)?;
            let mut client = Client::new(grantor.capture())?;
            let offer = client.describe()?.id();
            client.open_stream(1, offer, 1)?;
            client.register_destination(1, image(fixture)?)?;
            let retained = client.destination(1)?;
            let again = client.destination(1)?;
            check(core::ptr::eq(&*retained, &*again))?;
            client.unregister_destination(1)?;
            check(matches!(client.destination(1), Err(ENOENT)))?;
            check(client.register_destination(1, image(fixture)?) == Err(ESTALE))?;
            client.register_destination(2, image(fixture)?)?;
            check(!core::ptr::eq(&*retained, &*client.destination(2)?))?;
            client.close_stream(1)?;
            drop(client);
            check(retained.dimensions() == [640, 480])?;
            check(retained.buffer().size() >= 640 * 480 * 4)
        })
    }

    #[test]
    fn bounded_registration_retries_failed_names_and_preserves_cleanup() -> Result {
        with_exporter(|fixture| {
            let _connector = fixture.drm.publish_connector_identity()?;
            let creator = fixture.drm.master_file()?;
            let _fb = select(fixture, &creator)?;
            let grantor = grant(fixture, &creator)?;
            let mut client = Client::new(grantor.capture())?;
            check(client.register_destination(0, image(fixture)?) == Err(EINVAL))?;
            for id in 1..=16 {
                client.register_destination(id, image(fixture)?)?;
            }
            check(client.register_destination(17, image(fixture)?) == Err(EBUSY))?;
            client.unregister_destination(1)?;
            client.register_destination(17, image(fixture)?)?;
            drop(grantor);
            check(client.register_destination(18, image(fixture)?) == Err(EKEYREVOKED))?;
            for id in 2..=17 {
                client.unregister_destination(id)?;
            }
            check(client.unregister_destination(17) == Err(ENOENT))
        })
    }

    #[test]
    fn names_are_local_to_each_capture_client() -> Result {
        with_exporter(|fixture| {
            let _connector = fixture.drm.publish_connector_identity()?;
            let creator = fixture.drm.master_file()?;
            let _fb = select(fixture, &creator)?;
            let grantor = grant(fixture, &creator)?;
            let mut first = Client::new(grantor.capture())?;
            let mut second = Client::new(grantor.capture())?;
            first.register_destination(7, image(fixture)?)?;
            second.register_destination(7, image(fixture)?)?;
            let retained = second.destination(7)?;
            first.unregister_destination(7)?;
            check(core::ptr::eq(&*retained, &*second.destination(7)?))?;
            second.unregister_destination(7)
        })
    }

    #[test]
    fn read_only_storage_does_not_consume_a_destination_name() -> Result {
        with_exporter(|fixture| {
            let _connector = fixture.drm.publish_connector_identity()?;
            let creator = fixture.drm.master_file()?;
            let _fb = select(fixture, &creator)?;
            let grantor = grant(fixture, &creator)?;
            let mut client = Client::new(grantor.capture())?;
            let buffer = fixture.drm.export_dumb_read_only(640, 480, 32)?;
            check(!buffer.is_writable())?;
            let read_only = Image::new(
                buffer,
                Layout::new(640, 480)?,
                fourcc::XRGB8888,
                fourcc::FORMAT_MOD_LINEAR,
                2560,
                0,
            )?;
            check(client.register_destination(1, read_only) == Err(EACCES))?;
            client.register_destination(1, image(fixture)?)
        })
    }

    #[test]
    fn a_small_view_can_reuse_a_larger_bounded_allocation() -> Result {
        with_exporter(|fixture| {
            let _connector = fixture.drm.publish_connector_identity()?;
            let creator = fixture.drm.master_file()?;
            let _fb = select(fixture, &creator)?;
            let grantor = grant(fixture, &creator)?;
            let mut client = Client::new(grantor.capture())?;
            let larger = Image::new(
                fixture.drm.export_dumb(2048, 2049, 32)?,
                Layout::new(640, 480)?,
                fourcc::XRGB8888,
                fourcc::FORMAT_MOD_LINEAR,
                2560,
                0,
            )?;
            client.register_destination(1, larger)?;
            client.unregister_destination(1)
        })
    }
}
