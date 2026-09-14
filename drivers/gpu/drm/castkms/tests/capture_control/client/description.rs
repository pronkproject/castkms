// SPDX-License-Identifier: GPL-2.0-only

//! File callbacks use the same configuration and permission checks as kernel negotiation.

use super::*;
use crate::capture::client::Client;
use kernel::drm::{
    capture::Description,
    fourcc, //
};

fn files(grantor: Grantor) -> Result<(ClientFile, ControlFile)> {
    let (client, control) = grantor
        .into_files_with(|capture| Ok(Client::new(capture)))?
        .into_files();
    Ok((ClientFile(Some(client)), ControlFile(Some(control))))
}

fn describe(client: &ClientFile) -> Result<Description> {
    Description::query(client.0.as_ref().ok_or(EINVAL)?)
}

#[kunit_tests(rust_castkms_capture_file_description)]
mod cases {
    use super::*;

    #[test]
    fn callbacks_describe_layout_without_starting_composition() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let creator = fixture.drm.master_file()?;
        let fb = select(&fixture, &creator)?;
        let (client, _control) = files(grant(&fixture, &creator)?)?;
        let first = describe(&client)?;
        check(first.id() == 1 && first.dimensions() == [640, 480])?;
        check(first.format() == fourcc::XRGB8888)?;
        check(first.modifier() == fourcc::FORMAT_MOD_LINEAR)?;
        check(first.max_requests() == 8)?;
        check(describe(&client)? == first)?;
        fixture.select(&fb, false, 0)?;
        check(describe(&client)? == first)?;
        check(matches!(fixture.drm.device().host.current(), Err(EAGAIN)))?;
        fixture.drm.update(|transaction| {
            transaction
                .add_crtc_state(fixture.drm.crtc()?)?
                .set_mode_changed(true);
            Ok(())
        })?;
        let next = describe(&client)?;
        check(next.id() == first.id() + 1)?;
        check(next.dimensions() == first.dimensions())
    }

    #[test]
    fn callbacks_recheck_creator_revocation_after_a_successful_query() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let creator = fixture.drm.master_file()?;
        let _fb = select(&fixture, &creator)?;
        let (client, _control) = files(grant(&fixture, &creator)?)?;
        let _description = describe(&client)?;
        drop(creator);
        check(describe(&client) == Err(EKEYREVOKED))
    }

    #[test]
    fn client_close_leaves_another_clients_offer_authorized() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let creator = fixture.drm.master_file()?;
        let _fb = select(&fixture, &creator)?;
        let grantor = grant(&fixture, &creator)?;
        let sibling = ClientFile(Some(
            grantor
                .capture()
                .into_client_file_with(|capture| Ok(Client::new(capture)))?,
        ));
        let (client, control) = files(grantor)?;
        let first = describe(&client)?;
        check(describe(&sibling)? == first)?;
        drop(client);
        check(describe(&sibling)? == first)?;
        drop(control);
        check(describe(&sibling) == Err(EKEYREVOKED))
    }

    #[test]
    fn registered_issuance_installs_the_description_provider() -> Result {
        let display = CastKms::new(c"castkms-described-file")?;
        let dev = display._display.registration_guard().ok_or(ENODEV)?;
        let creator = RegisteredMasterFile::new(&dev)?;
        let target = registered_target(&creator)?;
        let (client, control) = dev
            .create_capture_grant(creator.file(), target)?
            .into_files();
        let client = ClientFile(Some(client));
        let _control = ControlFile(Some(control));
        // The target has no active mode; the installed provider still checks output state.
        check(describe(&client) == Err(ENODEV))?;
        drop(creator);
        check(describe(&client) == Err(EKEYREVOKED))
    }
}
