// SPDX-License-Identifier: GPL-2.0-only

//! Client stream names select independent queues without replacing their configuration.

use super::*;
use crate::capture::client::Client;
use kernel::drm::capture::ClientOwner;

#[kunit_tests(rust_castkms_capture_client_streams)]
mod cases {
    use super::*;

    #[test]
    fn closing_returns_capacity_without_reusing_the_stream_name() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let creator = fixture.drm.master_file()?;
        let _fb = select(&fixture, &creator)?;
        let grantor = grant(&fixture, &creator)?;
        let mut client = Client::new(grantor.capture())?;
        let offer = client.describe()?.id();
        for id in 1..=16 {
            client.open_stream(id, offer, 1)?;
        }
        check(client.open_stream(17, offer, 1) == Err(EBUSY))?;
        client.stream(1)?.queue(1)?;
        client.close_stream(1)?;
        check(matches!(client.stream(1), Err(ENOENT)))?;
        check(client.close_stream(1) == Err(ENOENT))?;
        check(client.close_stream(0) == Err(EINVAL))?;
        check(client.open_stream(1, offer, 1) == Err(ESTALE))?;
        client.open_stream(17, offer, 1)?;
        client.stream(17)?.queue(1)?;
        fixture.drm.device().host.current()?.flush_for_test();
        check(client.stream(17)?.advance()? == 1)?;
        client.stream(17)?.dequeue(|completion| {
            check(completion.result?.metadata().layout().dimensions() == (640, 480))
        })
    }

    #[test]
    fn closing_a_stream_keeps_its_sibling_live_and_cleanup_available() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let creator = fixture.drm.master_file()?;
        let _fb = select(&fixture, &creator)?;
        let grantor = grant(&fixture, &creator)?;
        let mut client = Client::new(grantor.capture())?;
        let offer = client.describe()?.id();
        client.open_stream(1, offer, 1)?;
        client.open_stream(2, offer, 1)?;
        client.stream(1)?.queue(1)?;
        client.close_stream(1)?;
        client.stream(2)?.queue(1)?;
        fixture.drm.device().host.current()?.flush_for_test();
        check(client.stream(2)?.advance()? == 1)?;
        client.stream(2)?.dequeue(|completion| {
            check(completion.result?.metadata().layout().dimensions() == (640, 480))
        })?;
        drop(grantor);
        check(client.open_stream(3, offer, 1) == Err(EKEYREVOKED))?;
        client.close_stream(2)?;
        check(matches!(client.stream(2), Err(ENOENT)))
    }

    #[test]
    fn opening_and_queueing_independent_named_streams() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let creator = fixture.drm.master_file()?;
        let _fb = select(&fixture, &creator)?;
        let grantor = grant(&fixture, &creator)?;
        let mut client = Client::new(grantor.capture())?;
        check(client.open_stream(10, 1, 1) == Err(ESTALE))?;
        let offer = client.describe()?.id();
        check(client.open_stream(0, offer, 1) == Err(EINVAL))?;
        check(client.open_stream(10, offer + 1, 1) == Err(ESTALE))?;
        check(client.open_stream(10, offer, 0) == Err(EINVAL))?;
        check(matches!(fixture.drm.device().host.current(), Err(EAGAIN)))?;
        client.open_stream(10, offer, 1)?;
        check(client.open_stream(10, offer, 1) == Err(ESTALE))?;
        client.open_stream(20, offer, 1)?;
        check(matches!(client.stream(0), Err(EINVAL)))?;
        check(matches!(client.stream(11), Err(ENOENT)))?;
        client.stream(10)?.queue(1)?;
        client.stream(20)?.queue(1)?;
        fixture.drm.device().host.current()?.flush_for_test();
        for id in [20, 10] {
            let mut queue = client.stream(id)?;
            check(queue.advance()? == 1)?;
            queue.dequeue(|completion| {
                check(completion.use_id == 1)?;
                check(completion.result?.metadata().layout().dimensions() == (640, 480))
            })?;
        }
        Ok(())
    }

    #[test]
    fn client_drop_releases_its_bounded_stream_storage() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let creator = fixture.drm.master_file()?;
        let _fb = select(&fixture, &creator)?;
        let grantor = grant(&fixture, &creator)?;
        let mut client = Client::new(grantor.capture())?;
        let offer = client.describe()?.id();
        for id in 1..=16 {
            client.open_stream(id, offer, 1)?;
        }
        check(client.open_stream(17, offer, 1) == Err(EBUSY))?;
        drop(client);
        let mut replacement = Client::new(grantor.capture())?;
        let offer = replacement.describe()?.id();
        for id in 1..=16 {
            replacement.open_stream(id, offer, 1)?;
        }
        Ok(())
    }

    #[test]
    fn a_new_offer_does_not_replace_an_existing_queue() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let creator = fixture.drm.master_file()?;
        let _fb = select(&fixture, &creator)?;
        let grantor = grant(&fixture, &creator)?;
        let mut client = Client::new(grantor.capture())?;
        let old = client.describe()?.id();
        client.open_stream(1, old, 1)?;
        fixture.drm.update(|transaction| {
            transaction
                .add_crtc_state(fixture.drm.crtc()?)?
                .set_mode_changed(true);
            Ok(())
        })?;
        let fresh = client.describe()?.id();
        check(fresh != old)?;
        check(client.open_stream(2, old, 1) == Err(ESTALE))?;
        client.open_stream(2, fresh, 1)?;
        check(client.stream(1)?.queue(1) == Err(EKEYREVOKED))?;
        client.stream(2)?.queue(1)?;
        fixture.drm.device().host.current()?.flush_for_test();
        check(client.stream(2)?.advance()? == 1)?;
        client.stream(2)?.dequeue(|completion| {
            check(completion.result?.metadata().layout().dimensions() == (640, 480))
        })
    }

    #[test]
    fn exhausted_names_and_revocation_do_not_open_another_stream() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let creator = fixture.drm.master_file()?;
        let _fb = select(&fixture, &creator)?;
        let grantor = grant(&fixture, &creator)?;
        let mut client = Client::new(grantor.capture())?;
        let offer = client.describe()?.id();
        client.open_stream(u64::MAX, offer, 1)?;
        check(client.open_stream(1, offer, 1) == Err(EOVERFLOW))?;
        drop(client);
        let mut client = Client::new(grantor.capture())?;
        let offer = client.describe()?.id();
        drop(grantor);
        check(client.open_stream(1, offer, 1) == Err(EKEYREVOKED))?;
        check(matches!(client.stream(1), Err(ENOENT)))
    }
}
