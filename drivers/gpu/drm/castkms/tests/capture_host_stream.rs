// SPDX-License-Identifier: GPL-2.0-only

//! Authorized kernel capture driven by the shared host worker.

mod pending;
mod progress;
mod queue;

use super::*;
use crate::{
    capture::{
        host_stream::Stream,
        permission::Permission,
        provider::{
            Grantor,
            Request, //
        }, //
    },
    host_compositor::layout::Layout, //
};
use kernel::{
    dma_fence::testing::ManualFence,
    drm::{
        capture::Status,
        kms::testing::MasterFile,
        preparation::Source, //
    },
    io::{
        io_project,
        Io, //
    },
    sync::{
        aref::ARef,
        Arc,
        Mutex, //
    },
    workqueue::{
        self,
        impl_has_work,
        new_work,
        Work,
        WorkItem, //
    }, //
};

fn grant(fixture: &Fixture, file: &MasterFile<'_, Driver>) -> Result<Grantor> {
    let permission = {
        let snapshot = file.file().master_snapshot().ok_or(EINVAL)?;
        let guard = snapshot.master().lock_current().ok_or(EACCES)?;
        Permission::new(&guard, fixture.drm.crtc()?, fixture.drm.connector()?)?
    };
    Grantor::new(permission)
}

fn select(fixture: &Fixture, file: &MasterFile<'_, Driver>) -> Result<FramebufferRef<Driver>> {
    let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(
        file.file().master_snapshot(),
    ))?;
    fixture.select(&fb, false, 0)?;
    Ok(fb)
}

#[pin_data]
struct CaptureTask {
    #[pin]
    work: Work<Self>,
    #[pin]
    stream: Mutex<Stream>,
    #[pin]
    result: Mutex<Option<Result<Request>>>,
}

impl_has_work! {
    impl HasWork<Self> for CaptureTask { self.work }
}

impl CaptureTask {
    fn new(stream: Stream) -> Result<Arc<Self>> {
        Arc::pin_init(
            pin_init!(Self {
                work <- new_work!("castkms-capture-host-test"),
                stream <- kernel::new_mutex!(stream),
                result <- kernel::new_mutex!(None),
            }),
            GFP_KERNEL,
        )
    }
}

impl WorkItem for CaptureTask {
    type Pointer = Arc<Self>;

    fn run(task: Arc<Self>) {
        let result = task.stream.lock().capture();
        *task.result.lock() = Some(result);
    }
}

#[kunit_tests(rust_castkms_capture_host_stream)]
mod cases {
    use super::*;

    #[test]
    fn capture_drives_composition_and_returns_independent_pixels() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let fb = select(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let mut stream = Stream::new(&grantor.capture(), 6)?;
        let mut results = KVec::new();
        for frame in 1..=6u8 {
            {
                let mapping = fb.vmap::<gem::Object>()?;
                io_project!(mapping.view(), [try: 0..2560]).copy_from_slice(&[frame; 2560]);
            }
            fixture.select(&fb, false, 0)?;
            let result = stream.capture()?;
            check(result.status()? == Status::Complete(Ok(())))?;
            results.push(result, GFP_KERNEL)?;
            let source: ARef<Source> = fixture
                .drm
                .device()
                .output
                .inspect_accepted(|accepted| accepted.map(|(source, _)| source.into()))
                .ok_or(EINVAL)?;
            let admission = source.hold_admission()?;
            check(admission.prepared()?.is_some())?;
        }
        check(matches!(stream.capture(), Err(EAGAIN)))?;
        fixture.drm.device().host.stop_worker()?;
        let mut pixels = KVVec::new();
        pixels.resize(Layout::new(640, 480)?.pixel_bytes(), 0xa7, GFP_KERNEL)?;
        for (index, result) in results.iter().enumerate() {
            check(result.copy_result(&mut pixels)? == pixels.len())?;
            let frame = index as u8 + 1;
            check(
                pixels[..2560]
                    .chunks_exact(4)
                    .all(|pixel| pixel == [frame, frame, frame, 0xff]),
            )?;
        }
        Ok(())
    }

    #[test]
    fn preparation_hold_leaves_capture_pending_until_admission_reopens() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = select(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let stream = Stream::new(&grantor.capture(), 1)?;
        let source: ARef<Source> = fixture
            .drm
            .device()
            .output
            .inspect_accepted(|accepted| accepted.map(|(source, _)| source.into()))
            .ok_or(EINVAL)?;
        let hold = source.hold_admission()?;
        let mut pending = stream.queue()?;
        fixture.drm.device().host.current()?.flush_for_test();
        check(pending.try_complete_frame()?.is_none())?;
        check(hold.prepared()?.is_some())?;
        drop(hold);
        let frame = pending.wait_frame()?;
        check(frame.request().status()? == Status::Complete(Ok(())))?;
        Ok(())
    }

    #[test]
    fn closed_stream_does_not_stop_a_siblings_worker() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = select(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let capture = grantor.capture();
        let mut first = Stream::new(&capture, 1)?;
        let mut second = Stream::new(&capture, 1)?;
        let discarded = first.capture()?;
        drop(first);
        check(discarded.status() == Err(ENOENT))?;
        check(second.capture()?.wait()? == Ok(()))?;
        drop(grantor);
        check(matches!(second.capture(), Err(EKEYREVOKED)))?;
        Ok(())
    }

    #[test]
    fn producer_failure_abandons_demand_without_consuming_queue_capacity() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let fb = select(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let mut stream = Stream::new(&grantor.capture(), 1)?;
        let mut producer = ManualFence::new()?;
        producer.complete(Err(EIO))?;
        fixture.select_with_producer(&fb, false, 0, Some(&producer.fence()))?;
        for _ in 0..16 {
            check(matches!(stream.capture(), Err(EIO)))?;
        }
        fixture.select(&fb, false, 0)?;
        check(stream.capture()?.wait()? == Ok(()))?;
        Ok(())
    }

    #[test]
    fn modeset_retirement_requires_a_fresh_stream() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let fb = select(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let capture = grantor.capture();
        let mut old = Stream::new(&capture, 1)?;
        drop(old.capture()?);
        fixture.drm.update(|mut transaction| {
            transaction
                .as_mut()
                .set_crtc_config(fixture.drm.crtc()?, None)
        })?;
        fixture.select(&fb, false, 0)?;
        check(matches!(old.capture(), Err(EKEYREVOKED)))?;
        let mut current = Stream::new(&capture, 1)?;
        check(current.capture()?.wait()? == Ok(()))?;
        Ok(())
    }

    #[test]
    fn worker_replacement_is_terminal_for_old_adapters() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = select(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let capture = grantor.capture();
        let mut old = Stream::new(&capture, 1)?;
        fixture.drm.device().host.stop_worker()?;
        for _ in 0..16 {
            check(matches!(old.capture(), Err(ENODEV)))?;
        }
        let mut fresh = Stream::new(&capture, 1)?;
        check(fresh.capture()?.wait()? == Ok(()))?;
        check(matches!(old.capture(), Err(ENODEV)))?;
        Ok(())
    }

    #[test]
    fn master_close_and_shutdown_end_automatic_capture() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = select(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let capture = grantor.capture();
        let mut stream = Stream::new(&capture, 1)?;
        drop(file);
        check(matches!(stream.capture(), Err(EKEYREVOKED)))?;
        check(matches!(Stream::new(&capture, 1), Err(EACCES)))?;
        fixture.state.close();
        check(matches!(stream.capture(), Err(EKEYREVOKED)))?;
        Ok(())
    }

    #[test]
    fn independent_streams_capture_concurrently_without_lost_results() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = select(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let capture = grantor.capture();
        let tasks = [
            CaptureTask::new(Stream::new(&capture, 1)?)?,
            CaptureTask::new(Stream::new(&capture, 1)?)?,
        ];
        for _ in 0..32 {
            let mut queued = true;
            for task in &tasks {
                queued &= workqueue::system_dfl().enqueue(task.clone()).is_ok();
            }
            // Always join both tasks before reporting a test failure or releasing the fixture.
            for task in &tasks {
                task.work.flush();
            }
            check(queued)?;
            for task in &tasks {
                let result = task.result.lock().take().ok_or(EINVAL)??;
                check(result.status()? == Status::Complete(Ok(())))?;
            }
        }
        Ok(())
    }
}
