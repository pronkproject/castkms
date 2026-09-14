// SPDX-License-Identifier: GPL-2.0-only

//! Submitted destination names retain their exact allocations through completion.

use super::*;
use crate::capture::client::Client;
use kernel::drm::capture::ClientOwner;
use kernel::{
    drm::capture::Readiness,
    time::{
        delay::fsleep,
        Delta,
        Instant,
        Monotonic, //
    }, //
};

fn wait_for_results(readiness: &Readiness) -> Result {
    let start = Instant::<Monotonic>::now();
    while !readiness.has_results() && start.elapsed() < Delta::from_millis(1000) {
        fsleep(Delta::from_millis(1));
    }
    check(readiness.has_results())
}

#[kunit_tests(rust_castkms_client_output)]
mod cases {
    use super::*;

    #[test]
    fn background_delivery_keeps_readiness_until_successful_dequeue() -> Result {
        with_exporter(|fixture| {
            let _connector = fixture.drm.publish_connector_identity()?;
            let file = fixture.drm.master_file()?;
            let _fb = select(fixture, &file)?;
            let grantor = grant(fixture, &file)?;
            let mut client = Client::new(grantor.capture())?;
            let readiness: ARef<Readiness> = client.readiness().ok_or(EINVAL)?.into();
            let offer = client.describe()?.id();
            for id in [1, 2] {
                client.open_stream(id, offer, 1)?;
                client.register_destination(id, destination(fixture, Layout::new(640, 480)?)?)?;
            }
            let first = client.destination(1)?;
            let second = client.destination(2)?;
            let mut reuse = ManualFence::new()?;
            client.queue_to(1, 1, 1, Some(reuse.fence()))?;
            client.queue_to(2, 1, 2, None)?;
            wait_for_results(&readiness)?;
            check(client.stream(2)?.dequeue::<()>(|_| Err(EFAULT)) == Err(EFAULT))?;
            check(readiness.has_results())?;
            client
                .stream(2)?
                .dequeue(|completion| completion.result.map(|_| ()))?;
            check(!readiness.has_results())?;
            let source: ARef<Source> = fixture
                .drm
                .device()
                .output
                .inspect_accepted(|accepted| accepted.map(|(source, _)| source.into()))
                .ok_or(EINVAL)?;
            {
                let admission = source.hold_admission()?;
                check(admission.prepared()?.is_some())?;
            }
            check(pixels(first.buffer())?.iter().all(|byte| *byte == 0x73))?;
            check(pixels(second.buffer())?[128..132] == [0x12, 0x12, 0x12, 0xff])?;
            reuse.complete(Ok(()))?;
            wait_for_results(&readiness)?;
            client
                .stream(1)?
                .dequeue(|completion| completion.result.map(|_| ()))?;
            check(!readiness.has_results())?;
            check(pixels(first.buffer())?[128..132] == [0x12, 0x12, 0x12, 0xff])?;
            drop(client);
            check(!readiness.has_results())
        })
    }

    #[test]
    fn closing_a_client_prevents_late_output_when_reuse_finishes() -> Result {
        with_exporter(|fixture| {
            let _connector = fixture.drm.publish_connector_identity()?;
            let file = fixture.drm.master_file()?;
            let _fb = select(fixture, &file)?;
            let grantor = grant(fixture, &file)?;
            let mut client = Client::new(grantor.capture())?;
            let readiness: ARef<Readiness> = client.readiness().ok_or(EINVAL)?.into();
            let offer = client.describe()?.id();
            client.open_stream(1, offer, 1)?;
            client.register_destination(1, destination(fixture, Layout::new(640, 480)?)?)?;
            let image = client.destination(1)?;
            let mut reuse = ManualFence::new()?;
            client.queue_to(1, 1, 1, Some(reuse.fence()))?;
            drop(client);
            reuse.complete(Ok(()))?;
            fixture.drm.device().host.current()?.flush_for_test();
            check(!readiness.has_results())?;
            check(pixels(image.buffer())?.iter().all(|byte| *byte == 0x73))
        })
    }

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
            check(client.stream(1)?.advance()? == 0)?;
            check(pixels(original.buffer())?.iter().all(|byte| *byte == 0x73))?;
            reuse.complete(Ok(()))?;
            client.stream(1)?.advance()?;
            wait_for_results(client.readiness().ok_or(EINVAL)?)?;
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
            fixture.drm.device().host.current()?.flush_for_test();
            client.stream(1)?.advance()?;
            wait_for_results(client.readiness().ok_or(EINVAL)?)?;
            client.stream(1)?.dequeue(|completion| {
                check(completion.use_id == 10)?;
                completion.result.map(|_| ())
            })?;
            drop(grantor);
            check(client.queue_to(1, 11, 2, None) == Err(EKEYREVOKED))?;
            client.unregister_destination(1)?;
            client.unregister_destination(2)?;
            client.close_stream(1)
        })
    }

    #[test]
    fn cancellation_uses_the_stream_not_the_removed_destination_name() -> Result {
        with_exporter(|fixture| {
            let _connector = fixture.drm.publish_connector_identity()?;
            let file = fixture.drm.master_file()?;
            let _fb = select(fixture, &file)?;
            let grantor = grant(fixture, &file)?;
            let mut client = Client::new(grantor.capture())?;
            let offer = client.describe()?.id();
            for id in [1, 2] {
                client.open_stream(id, offer, 1)?;
                client.register_destination(id, destination(fixture, Layout::new(640, 480)?)?)?;
            }
            let first = client.destination(1)?;
            let second = client.destination(2)?;
            let mut reuse = ManualFence::new()?;
            client.queue_to(1, 7, 1, Some(reuse.fence()))?;
            client.queue_to(2, 7, 2, None)?;
            client.unregister_destination(1)?;
            fixture.drm.device().host.current()?.flush_for_test();
            check(client.stream(1)?.advance()? == 0)?;
            client.stream(2)?.advance()?;
            wait_for_results(client.readiness().ok_or(EINVAL)?)?;
            check(client.cancel(3, 7) == Err(ENOENT))?;
            check(client.cancel(1, 8) == Err(ENOENT))?;
            client.cancel(1, 7)?;
            drop(grantor);
            client.stream(1)?.advance()?;
            check(client.cancel(1, 7) == Err(EALREADY))?;
            check(client.cancel(2, 7) == Err(EALREADY))?;
            client.stream(1)?.dequeue(|completion| {
                check(completion.use_id == 7)?;
                check(matches!(completion.result, Err(ECANCELED)))
            })?;
            client.stream(2)?.dequeue(|completion| {
                check(completion.use_id == 7)?;
                completion.result.map(|_| ())
            })?;
            reuse.complete(Ok(()))?;
            check(client.stream(1)?.advance()? == 0)?;
            check(pixels(first.buffer())?.iter().all(|byte| *byte == 0x73))?;
            check(pixels(second.buffer())?[128..132] == [0x12, 0x12, 0x12, 0xff])?;
            client.close_stream(1)?;
            check(client.cancel(1, 7) == Err(ENOENT))?;
            client.close_stream(2)?;
            client.unregister_destination(2)
        })
    }
}
