// SPDX-License-Identifier: GPL-2.0-only

//! Client accounting survives deferred completion and failed result publication.

use super::*;
use crate::capture::host_queue::Queue;

#[kunit_tests(rust_castkms_capture_host_queue)]
mod cases {
    use super::*;

    #[test]
    fn a_modeset_after_stream_allocation_preserves_the_newer_worker() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = select(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let description = grantor.capture().describe_stream()?;
        let delivery = description.create_stream(1)?;
        fixture.drm.update(|transaction| {
            transaction
                .add_crtc_state(fixture.drm.crtc()?)?
                .set_mode_changed(true);
            Ok(())
        })?;
        let device = fixture.drm.device();
        let newer = device.host.configure(device, Layout::new(3, 2)?)?;
        check(matches!(
            device
                .host
                .configure_checked(device, description.layout(), || delivery.check_current()),
            Err(ESTALE)
        ))?;
        newer.request()?;
        newer.flush_for_test();
        check(newer.take_outcome().is_some())
    }

    #[test]
    fn revocation_after_stream_allocation_does_not_allocate_a_worker() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = select(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let description = grantor.capture().describe_stream()?;
        let delivery = description.create_stream(1)?;
        drop(grantor);
        let device = fixture.drm.device();
        check(matches!(
            device
                .host
                .configure_checked(device, description.layout(), || delivery.check_current()),
            Err(EKEYREVOKED)
        ))?;
        check(matches!(device.host.current(), Err(EAGAIN)))
    }

    #[test]
    fn publication_failure_retains_the_result_and_its_credit() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = select(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let mut queue = Queue::new(&grantor.capture(), 2)?;
        check(queue.queue(0) == Err(EINVAL))?;
        queue.queue(10)?;
        queue.queue(20)?;
        check(queue.queue(30) == Err(EAGAIN))?;
        check(queue.dequeue(|_| Ok(())) == Err(EAGAIN))?;
        fixture.drm.device().host.current()?.flush_for_test();
        check(queue.advance() == 2)?;
        check(queue.advance() == 0)?;
        let failed: Result = queue.dequeue(|completion| {
            check(completion.use_id == 10)?;
            check(completion.result?.metadata().layout().dimensions() == (640, 480))?;
            Err(EFAULT)
        });
        check(failed == Err(EFAULT))?;
        check(queue.queue(30) == Err(EAGAIN))?;
        queue.dequeue(|completion| {
            check(completion.use_id == 10)?;
            let mut pixels = KVVec::new();
            pixels.resize(640 * 480 * 4, 0xa7, GFP_KERNEL)?;
            completion.result?.request().copy_result(&mut pixels)?;
            check(pixels.chunks_exact(4).all(|pixel| pixel == [0, 0, 0, 0xff]))
        })?;
        check(queue.queue(10) == Err(ESTALE))?;
        queue.queue(30)?;
        queue.dequeue(|completion| check(completion.use_id == 20))?;
        fixture.drm.device().host.current()?.flush_for_test();
        check(queue.advance() == 1)?;
        queue.dequeue(|completion| check(completion.use_id == 30))?;
        check(queue.dequeue(|_| Ok(())) == Err(EAGAIN))
    }

    #[test]
    fn revoked_attempts_still_need_terminal_acknowledgment() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = select(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let mut queue = Queue::new(&grantor.capture(), 2)?;
        queue.queue(1)?;
        queue.queue(2)?;
        drop(grantor);
        fixture.drm.device().host.current()?.flush_for_test();
        check(queue.advance() == 2)?;
        check(queue.queue(3) == Err(EAGAIN))?;
        check(queue.dequeue::<()>(|_| Err(EFAULT)) == Err(EFAULT))?;
        queue.dequeue(|completion| {
            check(completion.use_id == 1)?;
            check(matches!(completion.result, Err(EKEYREVOKED)))
        })?;
        queue.dequeue(|completion| {
            check(completion.use_id == 2)?;
            check(matches!(completion.result, Err(EKEYREVOKED)))
        })?;
        check(queue.dequeue(|_| Ok(())) == Err(EAGAIN))
    }

    #[test]
    fn exhausted_use_ids_require_a_new_stream_incarnation() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = select(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let mut queue = Queue::new(&grantor.capture(), 1)?;
        queue.queue(u64::MAX)?;
        fixture.drm.device().host.current()?.flush_for_test();
        check(queue.advance() == 1)?;
        queue.dequeue(|completion| check(completion.use_id == u64::MAX))?;
        check(queue.queue(1) == Err(EOVERFLOW))?;
        drop(queue);
        let mut replacement = Queue::new(&grantor.capture(), 1)?;
        replacement.queue(1)
    }
}
