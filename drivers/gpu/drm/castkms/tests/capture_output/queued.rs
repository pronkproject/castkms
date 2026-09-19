// SPDX-License-Identifier: GPL-2.0-only

//! Destination waits preserve private pixels without preserving compositor reads.

use super::*;
use crate::capture::host_queue::Queue;
use crate::capture::host_stream::output::Output;
use kernel::sync::Arc;
use kernel::time::{
    delay::fsleep,
    Delta,
    Instant,
    Monotonic, //
};

fn wait_output(output: &mut Output) -> Result<crate::capture::provider::Frame> {
    let start = Instant::<Monotonic>::now();
    loop {
        if let Some(frame) = output.try_complete_frame()? {
            return Ok(frame);
        }
        if start.elapsed() >= Delta::from_millis(1000) {
            return Err(ETIMEDOUT);
        }
        fsleep(Delta::from_millis(1));
    }
}

fn advance_one(queue: &mut Queue) -> Result {
    let start = Instant::<Monotonic>::now();
    loop {
        match queue.advance() {
            0 => (),
            1 => return Ok(()),
            _ => return Err(EINVAL),
        }
        if start.elapsed() >= Delta::from_millis(1000) {
            return Err(ETIMEDOUT);
        }
        fsleep(Delta::from_millis(1));
    }
}

#[kunit_tests(rust_castkms_capture_queued_output)]
mod cases {
    use super::*;

    #[test]
    fn ineligible_destination_does_not_consume_stream_capacity() -> Result {
        with_exporter(|fixture| {
            let _connector = fixture.drm.publish_connector_identity()?;
            let file = fixture.drm.master_file()?;
            let _fb = select(fixture, &file)?;
            let grantor = grant(fixture, &file)?;
            let stream = Stream::new(&grantor.capture(), 1)?;
            let wrong = Arc::new(destination(fixture, Layout::new(639, 480)?)?, GFP_KERNEL)?;
            check(matches!(stream.queue_to(wrong, None), Err(EINVAL)))?;
            let read_only = Arc::new(
                Image::new(
                    fixture.drm.export_dumb_read_only(640, 480, 32)?,
                    Layout::new(640, 480)?,
                    fourcc::XRGB8888,
                    fourcc::FORMAT_MOD_LINEAR,
                    2560,
                    0,
                )?,
                GFP_KERNEL,
            )?;
            check(matches!(stream.queue_to(read_only, None), Err(EACCES)))?;
            let image = Arc::new(destination(fixture, Layout::new(640, 480)?)?, GFP_KERNEL)?;
            let mut output = stream.queue_to(image.clone(), None)?;
            check(matches!(stream.queue_to(image.clone(), None), Err(EAGAIN)))?;
            fixture.drm.device().host.current()?.flush_for_test();
            let frame = wait_output(&mut output)?;
            check(frame.metadata().layout().dimensions() == (640, 480))?;
            check(image.dimensions() == [640, 480])?;
            check(pixels(image.buffer())?[128..132] == [0x12, 0x12, 0x12, 0xff])
        })
    }

    #[test]
    fn reuse_wait_retains_the_original_image_without_a_source_read() -> Result {
        with_exporter(|fixture| {
            let _connector = fixture.drm.publish_connector_identity()?;
            let file = fixture.drm.master_file()?;
            let fb = select(fixture, &file)?;
            let grantor = grant(fixture, &file)?;
            let stream = Stream::new(&grantor.capture(), 1)?;
            let image = Arc::new(destination(fixture, Layout::new(640, 480)?)?, GFP_KERNEL)?;
            let mut reuse = ManualFence::new()?;
            let mut output = Output::new(stream.queue()?, image.clone(), Some(reuse.fence()));
            fixture.drm.device().host.current()?.flush_for_test();
            check(output.try_complete_frame()?.is_none())?;
            check(pixels(image.buffer())?.iter().all(|byte| *byte == 0x73))?;
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
            {
                let map = fb.vmap::<gem::Object>()?;
                io_project!(map.view(), [try: 0..32]).copy_from_slice(&[0xdd; 32]);
            }
            fixture.select(&fb, false, 0)?;
            reuse.complete(Ok(()))?;
            let frame = wait_output(&mut output)?;
            check(frame.metadata().layout().dimensions() == (640, 480))?;
            check(image.dimensions() == [640, 480])?;
            check(pixels(image.buffer())?[128..132] == [0x12, 0x12, 0x12, 0xff])?;
            check(matches!(output.try_complete_frame(), Err(EALREADY)))
        })
    }

    #[test]
    fn failed_reuse_consumes_the_attempt_without_writing() -> Result {
        with_exporter(|fixture| {
            let _connector = fixture.drm.publish_connector_identity()?;
            let file = fixture.drm.master_file()?;
            let _fb = select(fixture, &file)?;
            let grantor = grant(fixture, &file)?;
            let stream = Stream::new(&grantor.capture(), 1)?;
            let image = Arc::new(destination(fixture, Layout::new(640, 480)?)?, GFP_KERNEL)?;
            let mut reuse = ManualFence::new()?;
            let mut output = Output::new(stream.queue()?, image.clone(), Some(reuse.fence()));
            fixture.drm.device().host.current()?.flush_for_test();
            check(output.try_complete_frame()?.is_none())?;
            reuse.complete(Err(EAGAIN))?;
            check(matches!(output.try_complete_frame(), Err(EAGAIN)))?;
            check(matches!(output.try_complete_frame(), Err(EALREADY)))?;
            check(pixels(image.buffer())?.iter().all(|byte| *byte == 0x73))?;
            let _replacement = stream.queue()?;
            Ok(())
        })
    }

    #[test]
    fn closing_a_stream_discards_a_private_image_waiting_for_reuse() -> Result {
        with_exporter(|fixture| {
            let _connector = fixture.drm.publish_connector_identity()?;
            let file = fixture.drm.master_file()?;
            let _fb = select(fixture, &file)?;
            let grantor = grant(fixture, &file)?;
            let stream = Stream::new(&grantor.capture(), 1)?;
            let image = Arc::new(destination(fixture, Layout::new(640, 480)?)?, GFP_KERNEL)?;
            let reuse = ManualFence::new()?;
            let mut output = Output::new(stream.queue()?, image.clone(), Some(reuse.fence()));
            fixture.drm.device().host.current()?.flush_for_test();
            check(output.try_complete_frame()?.is_none())?;
            drop(stream);
            check(matches!(output.try_complete_frame(), Err(ENOENT)))?;
            check(matches!(output.try_complete_frame(), Err(EALREADY)))?;
            check(pixels(image.buffer())?.iter().all(|byte| *byte == 0x73))
        })
    }

    #[test]
    fn ready_output_bypasses_reuse_wait_and_survives_failed_publication() -> Result {
        with_exporter(|fixture| {
            let _connector = fixture.drm.publish_connector_identity()?;
            let file = fixture.drm.master_file()?;
            let _fb = select(fixture, &file)?;
            let grantor = grant(fixture, &file)?;
            let mut queue = Queue::new(&grantor.capture(), 2)?;
            let first = Arc::new(destination(fixture, Layout::new(640, 480)?)?, GFP_KERNEL)?;
            let second = Arc::new(destination(fixture, Layout::new(640, 480)?)?, GFP_KERNEL)?;
            let mut reuse = ManualFence::new()?;
            queue.queue_to(1, first.clone(), Some(reuse.fence()))?;
            queue.queue_to(2, second.clone(), None)?;
            check(queue.queue_to(3, first.clone(), None) == Err(EBUSY))?;
            fixture.drm.device().host.current()?.flush_for_test();
            advance_one(&mut queue)?;
            check(pixels(first.buffer())?.iter().all(|byte| *byte == 0x73))?;
            check(pixels(second.buffer())?[128..132] == [0x12, 0x12, 0x12, 0xff])?;
            check(
                queue.dequeue::<()>(|completion| {
                    check(completion.use_id == 2)?;
                    completion.result?;
                    Err(EFAULT)
                }) == Err(EFAULT),
            )?;
            check(queue.queue_to(3, second.clone(), None) == Err(EBUSY))?;
            check(queue.advance() == 0)?;
            queue.dequeue(|completion| {
                check(completion.use_id == 2)?;
                check(completion.result?.metadata().layout().dimensions() == (640, 480))?;
                check(second.dimensions() == [640, 480])
            })?;
            reuse.complete(Ok(()))?;
            advance_one(&mut queue)?;
            queue.dequeue(|completion| {
                check(completion.use_id == 1)?;
                completion.result.map(|_| ())
            })?;
            check(pixels(first.buffer())?[128..132] == [0x12, 0x12, 0x12, 0xff])?;
            check(queue.queue_to(2, second.clone(), None) == Err(ESTALE))?;
            queue.queue_to(3, second, None)
        })
    }

    #[test]
    fn failed_output_keeps_its_terminal_record_until_acknowledged() -> Result {
        with_exporter(|fixture| {
            let _connector = fixture.drm.publish_connector_identity()?;
            let file = fixture.drm.master_file()?;
            let _fb = select(fixture, &file)?;
            let grantor = grant(fixture, &file)?;
            let mut queue = Queue::new(&grantor.capture(), 1)?;
            let image = Arc::new(destination(fixture, Layout::new(640, 480)?)?, GFP_KERNEL)?;
            let mut reuse = ManualFence::new()?;
            queue.queue_to(1, image.clone(), Some(reuse.fence()))?;
            reuse.complete(Err(EAGAIN))?;
            fixture.drm.device().host.current()?.flush_for_test();
            advance_one(&mut queue)?;
            check(queue.advance() == 0)?;
            check(queue.queue_to(2, image.clone(), None) == Err(EBUSY))?;
            check(queue.dequeue::<()>(|_| Err(EFAULT)) == Err(EFAULT))?;
            queue.dequeue(|completion| {
                check(completion.use_id == 1)?;
                check(matches!(completion.result, Err(EAGAIN)))
            })?;
            check(pixels(image.buffer())?.iter().all(|byte| *byte == 0x73))?;
            queue.queue_to(2, image, None)
        })
    }

    #[test]
    fn cancellation_never_writes_a_destination_after_reuse_finishes() -> Result {
        with_exporter(|fixture| {
            let _connector = fixture.drm.publish_connector_identity()?;
            let file = fixture.drm.master_file()?;
            let _fb = select(fixture, &file)?;
            let grantor = grant(fixture, &file)?;
            let stream = Stream::new(&grantor.capture(), 1)?;
            for capture_first in [false, true] {
                let image = Arc::new(destination(fixture, Layout::new(640, 480)?)?, GFP_KERNEL)?;
                let mut reuse = ManualFence::new()?;
                let mut output = stream.queue_to(image.clone(), Some(reuse.fence()))?;
                if capture_first {
                    fixture.drm.device().host.current()?.flush_for_test();
                    check(output.try_complete_frame()?.is_none())?;
                }
                output.cancel()?;
                check(output.cancel() == Err(EALREADY))?;
                reuse.complete(Ok(()))?;
                fixture.drm.device().host.current()?.flush_for_test();
                check(matches!(output.try_complete_frame(), Err(ECANCELED)))?;
                check(matches!(output.try_complete_frame(), Err(EALREADY)))?;
                check(pixels(image.buffer())?.iter().all(|byte| *byte == 0x73))?;
            }
            let image = Arc::new(destination(fixture, Layout::new(640, 480)?)?, GFP_KERNEL)?;
            let mut output = stream.queue_to(image.clone(), None)?;
            fixture.drm.device().host.current()?.flush_for_test();
            let _frame = wait_output(&mut output)?;
            check(output.cancel() == Err(EALREADY))?;
            check(pixels(image.buffer())?[128..132] == [0x12, 0x12, 0x12, 0xff])
        })
    }

    #[test]
    fn queue_cancellation_preserves_each_terminal_record() -> Result {
        with_exporter(|fixture| {
            let _connector = fixture.drm.publish_connector_identity()?;
            let file = fixture.drm.master_file()?;
            let _fb = select(fixture, &file)?;
            let grantor = grant(fixture, &file)?;
            let mut queue = Queue::new(&grantor.capture(), 2)?;
            let image = Arc::new(destination(fixture, Layout::new(640, 480)?)?, GFP_KERNEL)?;
            let mut reuse = ManualFence::new()?;
            queue.queue_to(1, image.clone(), Some(reuse.fence()))?;
            queue.queue(2)?;
            check(queue.cancel(0) == Err(EINVAL))?;
            check(queue.cancel(3) == Err(ENOENT))?;
            queue.cancel(2)?;
            fixture.drm.device().host.current()?.flush_for_test();
            advance_one(&mut queue)?;
            queue.cancel(1)?;
            check(queue.queue(3) == Err(EBUSY))?;
            advance_one(&mut queue)?;
            check(queue.cancel(1) == Err(EALREADY))?;
            check(queue.dequeue::<()>(|_| Err(EFAULT)) == Err(EFAULT))?;
            check(queue.queue(3) == Err(EBUSY))?;
            for id in [1, 2] {
                queue.dequeue(|completion| {
                    check(completion.use_id == id)?;
                    check(matches!(completion.result, Err(ECANCELED)))
                })?;
            }
            reuse.complete(Ok(()))?;
            check(queue.advance() == 0)?;
            check(pixels(image.buffer())?.iter().all(|byte| *byte == 0x73))?;
            check(queue.cancel(1) == Err(ENOENT))?;
            queue.queue_to(3, image, None)
        })
    }
}
