// SPDX-License-Identifier: GPL-2.0-only

//! Submitted destination names retain their exact allocations through completion.

use super::*;
use crate::capture::client::Client;
use kernel::drm::capture::ClientOwner;

#[kunit_tests(rust_castkms_client_output)]
mod cases {
    use super::*;

    #[test]
    fn removing_a_destination_name_does_not_replace_accepted_storage() -> Result {
        with_exporter(|fixture| {
            let _connector = fixture.drm.publish_connector_identity()?;
            let file = fixture.drm.master_file()?;
            let _fb = select(fixture, &file)?;
            let grantor = grant(fixture, &file)?;
            let mut client = Client::new(grantor.capture())?;
            let offer = client.describe()?.id();
            client.open_stream(1, offer, 1)?;
            client.register_destination(11, destination(fixture, Layout::new(640, 480)?)?)?;
            let original = client.destination(11)?;
            let mut reuse = ManualFence::new()?;
            client.queue_to(1, 1, 11, Some(reuse.fence()))?;
            client.unregister_destination(11)?;
            client.register_destination(12, destination(fixture, Layout::new(640, 480)?)?)?;
            let replacement = client.destination(12)?;
            check(client.queue_to(1, 2, 11, None) == Err(ENOENT))?;
            fixture.drm.device().host.current()?.flush_for_test();
            check(client.stream(1)?.advance() == 0)?;
            check(pixels(original.buffer())?.iter().all(|byte| *byte == 0x73))?;
            reuse.complete(Ok(()))?;
            check(client.stream(1)?.advance() == 1)?;
            client.stream(1)?.dequeue(|completion| {
                check(completion.use_id == 1)?;
                completion.result.map(|_| ())
            })?;
            check(pixels(original.buffer())?[128..132] == [0x12, 0x12, 0x12, 0xff])?;
            check(
                pixels(replacement.buffer())?
                    .iter()
                    .all(|byte| *byte == 0x73),
            )?;
            client.queue_to(1, 2, 12, None)
        })
    }

    #[test]
    fn rejected_binding_leaves_the_request_name_retryable() -> Result {
        with_exporter(|fixture| {
            let _connector = fixture.drm.publish_connector_identity()?;
            let file = fixture.drm.master_file()?;
            let _fb = select(fixture, &file)?;
            let grantor = grant(fixture, &file)?;
            let mut client = Client::new(grantor.capture())?;
            let offer = client.describe()?.id();
            client.open_stream(1, offer, 1)?;
            client.register_destination(1, destination(fixture, Layout::new(639, 480)?)?)?;
            client.register_destination(2, destination(fixture, Layout::new(640, 480)?)?)?;
            check(client.queue_to(2, 10, 2, None) == Err(ENOENT))?;
            check(client.queue_to(1, 10, 3, None) == Err(ENOENT))?;
            check(client.queue_to(1, 10, 1, None) == Err(EINVAL))?;
            client.queue_to(1, 10, 2, None)?;
            check(client.queue_to(1, 10, 2, None) == Err(ESTALE))?;
            drop(grantor);
            check(client.stream(1)?.advance() == 1)?;
            client.stream(1)?.dequeue(|completion| {
                check(completion.use_id == 10)?;
                check(matches!(completion.result, Err(EKEYREVOKED)))
            })?;
            check(client.queue_to(1, 11, 2, None) == Err(EKEYREVOKED))?;
            client.unregister_destination(1)?;
            client.unregister_destination(2)?;
            client.close_stream(1)
        })
    }
}
