// SPDX-License-Identifier: GPL-2.0-only

//! Independently queued attempts, with stream close separate from retained operation storage.

use super::*;
use crate::capture::host_stream::Pending;
use kernel::{
    drm::gem::IntoGEMObject,
    sync::Completion,
    time::{
        delay::fsleep,
        Delta,
        Instant,
        Monotonic, //
    },
    types::ScopeGuard, //
};

fn with_mapping_blocked<T>(
    fb: &FramebufferRef<Driver>,
    f: impl FnOnce() -> Result<T>,
) -> Result<T> {
    let object = fb.object::<gem::Object>()?;
    // SAFETY: The framebuffer retains the initialized object and its reservation throughout
    // this callback. Only one reservation is locked, without an outer acquire context.
    let reservation = unsafe { (*object.as_raw()).resv };
    // SAFETY: The retained object keeps this reservation live, and no other lock is held.
    kernel::error::to_result(unsafe {
        kernel::bindings::dma_resv_lock(reservation, core::ptr::null_mut())
    })?;
    let _unlock = ScopeGuard::new(|| {
        // SAFETY: Balance the lock before releasing the framebuffer borrow on every exit.
        unsafe { kernel::bindings::dma_resv_unlock(reservation) };
    });
    f()
}

#[pin_data]
struct PendingTask {
    #[pin]
    work: Work<Self>,
    #[pin]
    started: Completion,
    #[pin]
    pending: Mutex<Option<Pending>>,
    #[pin]
    result: Mutex<Option<Result<Request>>>,
}

impl_has_work! {
    impl HasWork<Self> for PendingTask { self.work }
}

impl PendingTask {
    fn new(pending: Pending) -> Result<Arc<Self>> {
        Arc::pin_init(
            pin_init!(Self {
                work <- new_work!("castkms-pending-cancel-test"),
                started <- Completion::new(),
                pending <- kernel::new_mutex!(Some(pending)),
                result <- kernel::new_mutex!(None),
            }),
            GFP_KERNEL,
        )
    }
}

impl WorkItem for PendingTask {
    type Pointer = Arc<Self>;

    fn run(task: Arc<Self>) {
        let pending = task.pending.lock().take();
        task.started.complete_all();
        let result = pending.ok_or(EALREADY).and_then(Pending::wait);
        *task.result.lock() = Some(result);
    }
}

#[kunit_tests(rust_castkms_capture_pending)]
mod cases {
    use super::*;

    #[test]
    fn closed_demand_is_observable_before_composition_can_map() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let fb = select(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let stream = Stream::new(&grantor.capture(), 1)?;
        let worker = fixture.drm.device().host.current()?;
        let results = with_mapping_blocked(&fb, || {
            let mut pending = stream.queue()?;
            drop(stream);
            Ok((pending.try_complete_frame(), pending.try_complete_frame()))
        })?;
        worker.flush_for_test();
        check(matches!(results.0, Err(ENOENT)))?;
        check(matches!(results.1, Err(EALREADY)))
    }

    #[test]
    fn revoked_demand_wakes_while_composition_is_blocked_on_mapping() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let fb = select(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let stream = Stream::new(&grantor.capture(), 1)?;
        let worker = fixture.drm.device().host.current()?;
        let (task, completed) = with_mapping_blocked(&fb, || {
            let task = PendingTask::new(stream.queue()?)?;
            check(workqueue::system_dfl().enqueue(task.clone()).is_ok())?;
            task.started.wait_for_completion();
            drop(grantor);
            let start = Instant::<Monotonic>::now();
            while task.result.lock().is_none() && start.elapsed() < Delta::from_millis(1000) {
                fsleep(Delta::from_millis(1));
            }
            let completed = task.result.lock().is_some();
            Ok((task, completed))
        })?;
        // Release mapping exclusion and join both tasks even if cancellation did not wake.
        task.work.flush();
        worker.flush_for_test();
        check(completed)?;
        check(matches!(task.result.lock().take(), Some(Err(EKEYREVOKED))))?;
        worker.request()?;
        worker.flush_for_test();
        check(worker.last_image().is_some())
    }

    #[test]
    fn deferred_completion_does_not_relabel_an_older_image() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let fb = select(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let stream = Stream::new(&grantor.capture(), 2)?;
        let device = fixture.drm.device();
        let worker = device.host.current()?;
        let mut pending = stream.queue()?;
        worker.flush_for_test();
        let image = worker.last_image().ok_or(EINVAL)?;
        let serial = image.content_serial();
        let time = image.completed_at();
        drop(image);
        fixture.select(&fb, false, 0)?;
        let frame = pending.try_complete_frame()?.ok_or(EINVAL)?;
        check(frame.metadata().content_serial() == serial)?;
        check(frame.metadata().completed_at() - time == kernel::time::Delta::ZERO)?;
        let current = stream.queue()?.wait_frame()?;
        check(current.metadata().content_serial() != serial)?;
        check(frame.request().status()? == Status::Complete(Ok(())))
    }

    #[test]
    fn reverse_completion_returns_each_queued_request() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = select(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let stream = Stream::new(&grantor.capture(), 6)?;
        let mut pending = KVec::new();
        for _ in 0..6 {
            pending.push(stream.queue()?, GFP_KERNEL)?;
        }
        check(matches!(stream.queue(), Err(EAGAIN)))?;
        let mut results = KVec::new();
        while let Some(operation) = pending.pop() {
            let result = operation.wait()?;
            check(result.status()? == Status::Complete(Ok(())))?;
            results.push(result, GFP_KERNEL)?;
        }
        let source: ARef<Source> = fixture
            .drm
            .device()
            .output
            .inspect_accepted(|accepted| accepted.map(|(source, _)| source.into()))
            .ok_or(EINVAL)?;
        check(source.hold_admission()?.prepared()?.is_some())?;
        drop(results);
        check(stream.queue()?.wait()?.status()? == Status::Complete(Ok(())))
    }

    #[test]
    fn dropping_one_attempt_releases_only_its_credit() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = select(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let stream = Stream::new(&grantor.capture(), 2)?;
        let first = stream.queue()?;
        let second = stream.queue()?;
        drop(first);
        let replacement = stream.queue()?;
        check(second.wait()?.status()? == Status::Complete(Ok(())))?;
        check(replacement.wait()?.status()? == Status::Complete(Ok(())))
    }

    #[test]
    fn completion_is_consumed_once_without_another_wait() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = select(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let stream = Stream::new(&grantor.capture(), 1)?;
        let device = fixture.drm.device();
        let worker = device.host.configure(device, Layout::new(640, 480)?)?;
        let mut pending = stream.queue()?;
        worker.flush_for_test();
        let request = pending.try_complete()?.ok_or(EINVAL)?;
        check(request.status()? == Status::Complete(Ok(())))?;
        check(matches!(pending.try_complete(), Err(EALREADY)))?;
        check(matches!(pending.wait(), Err(EALREADY)))?;
        Ok(())
    }

    #[test]
    fn closed_stream_rejects_retained_attempt_but_not_sibling() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = select(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let first = Stream::new(&grantor.capture(), 1)?;
        let second = Stream::new(&grantor.capture(), 1)?;
        let pending = first.queue()?;
        drop(first);
        check(matches!(pending.wait(), Err(ENOENT)))?;
        check(second.queue()?.wait()?.status()? == Status::Complete(Ok(())))
    }

    #[test]
    fn failed_worker_completion_consumes_demand_and_returns_credit() -> Result {
        let fixture = Fixture::new()?;
        let _connector = fixture.drm.publish_connector_identity()?;
        let file = fixture.drm.master_file()?;
        let _fb = select(&fixture, &file)?;
        let grantor = grant(&fixture, &file)?;
        let stream = Stream::new(&grantor.capture(), 1)?;
        let mut pending = stream.queue()?;
        fixture.drm.device().host.stop_worker()?;
        check(matches!(pending.try_complete(), Err(ENODEV)))?;
        check(matches!(pending.try_complete(), Err(EALREADY)))?;
        check(matches!(stream.queue(), Err(ENODEV)))?;
        let replacement = Stream::new(&grantor.capture(), 1)?;
        check(replacement.queue()?.wait()?.status()? == Status::Complete(Ok(())))
    }
}
