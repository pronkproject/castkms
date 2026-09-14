// SPDX-License-Identifier: GPL-2.0-only

//! File dispatch preserves kernel stream ownership, names and revoked cleanup.

use super::*;
use crate::capture::client::Client;
use kernel::drm::capture::{
    ClientStream,
    Description, //
};

#[kunit_tests(rust_castkms_capture_client_stream_files)]
mod cases {
    use super::*;

    #[test]
    fn file_streams_return_capacity_without_reusing_names() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let creator = fixture.drm.master_file()?;
        let _fb = select(&fixture, &creator)?;
        let (client, control) = grant(&fixture, &creator)?
            .into_files_with(Client::new)?
            .into_files();
        let client = ClientFile(Some(client));
        let _control = ControlFile(Some(control));
        let file = client.0.as_ref().ok_or(EINVAL)?;
        let offer = Description::query(file)?.id();
        let mut streams = KVec::new();
        for id in 1..=16 {
            streams.push(ClientStream::open(file, id, offer, 1)?, GFP_KERNEL)?;
        }
        check(matches!(ClientStream::open(file, 17, offer, 1), Err(EBUSY)))?;
        streams[0].close()?;
        check(matches!(ClientStream::open(file, 1, offer, 1), Err(ESTALE)))?;
        let _replacement = ClientStream::open(file, 17, offer, 1)?;
        Ok(())
    }

    #[test]
    fn failed_open_preserves_the_name_and_revocation_preserves_close() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let creator = fixture.drm.master_file()?;
        let _fb = select(&fixture, &creator)?;
        let (client, control) = grant(&fixture, &creator)?
            .into_files_with(Client::new)?
            .into_files();
        let client = ClientFile(Some(client));
        let control = ControlFile(Some(control));
        let file = client.0.as_ref().ok_or(EINVAL)?;
        let offer = Description::query(file)?.id();
        check(matches!(
            ClientStream::open(file, 1, offer + 1, 1),
            Err(ESTALE)
        ))?;
        check(matches!(ClientStream::open(file, 1, offer, 0), Err(EINVAL)))?;
        let mut stream = ClientStream::open(file, 1, offer, 1)?;
        drop(control);
        check(matches!(
            ClientStream::open(file, 2, offer, 1),
            Err(EKEYREVOKED)
        ))?;
        stream.close()?;
        check(stream.id().is_none())
    }

    #[test]
    fn file_streams_keep_cleanup_across_a_replaced_offer() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let creator = fixture.drm.master_file()?;
        let _fb = select(&fixture, &creator)?;
        let (client, control) = grant(&fixture, &creator)?
            .into_files_with(Client::new)?
            .into_files();
        let client = ClientFile(Some(client));
        let _control = ControlFile(Some(control));
        let file = client.0.as_ref().ok_or(EINVAL)?;
        let old = Description::query(file)?.id();
        let mut stream = ClientStream::open(file, 1, old, 1)?;
        fixture.drm.update(|transaction| {
            transaction
                .add_crtc_state(fixture.drm.crtc()?)?
                .set_mode_changed(true);
            Ok(())
        })?;
        let fresh = Description::query(file)?.id();
        check(fresh != old)?;
        check(matches!(ClientStream::open(file, 2, old, 1), Err(ESTALE)))?;
        let _replacement = ClientStream::open(file, 2, fresh, 1)?;
        stream.close()
    }
}
