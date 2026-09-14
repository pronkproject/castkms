// SPDX-License-Identifier: GPL-2.0-only

//! Destination waits preserve private pixels without preserving compositor reads.

use super::*;
use crate::capture::host_stream::output::Output;
use kernel::sync::Arc;

#[kunit_tests(rust_castkms_capture_queued_output)]
mod cases {
    use super::*;

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
            let frame = output.try_complete_frame()?.ok_or(EINVAL)?;
            check(frame.metadata().layout() == image.layout())?;
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
}
