// SPDX-License-Identifier: GPL-2.0-only

//! File submission uses the same retained destinations and terminal records as kernel clients.

use super::*;
use crate::capture::{
    client::Client,
    provider::Capture, //
};
use kernel::{
    drm::capture::{
        ClientDestination,
        ClientStream,
        Description,
        Destination,
        DestinationPlane,
        Readiness, //
    },
    time::{
        delay::fsleep,
        Delta,
        Instant,
        Monotonic, //
    },
    types::ScopeGuard, //
};

pub(super) fn with_client(
    capture: Capture,
    run: impl FnOnce(&kernel::fs::File) -> Result,
) -> Result {
    let file = capture.into_client_file_with(Client::new)?;
    let file = ScopeGuard::new_with_data(file, |file| {
        // SAFETY: The KUnit task releases its owned reference outside driver locks, after
        // the callback's stream and destination handles have completed their cleanup.
        unsafe { kernel::bindings::__fput_sync(ARef::into_raw(file).cast().as_ptr()) };
    });
    run(&file)
}

pub(super) fn register(
    file: &kernel::fs::File,
    id: u64,
    image: &Image,
) -> Result<ClientDestination> {
    let [width, height] = image.dimensions();
    let description = Destination::new(
        [width, height],
        fourcc::XRGB8888,
        fourcc::FORMAT_MOD_LINEAR,
        &[DestinationPlane::new(
            image.buffer(),
            image.pitch() as u32,
            image.offset() as u64,
        )],
    )?;
    ClientDestination::register(file, id, &description)
}

fn wait(readiness: &Readiness) -> Result {
    let start = Instant::<Monotonic>::now();
    while !readiness.has_results() {
        if start.elapsed() > Delta::from_millis(1000) {
            return Err(ETIMEDOUT);
        }
        fsleep(Delta::from_millis(1));
    }
    Ok(())
}

#[kunit_tests(rust_castkms_capture_file_output)]
mod cases {
    use super::*;

    #[test]
    fn file_submission_retains_storage_and_retryable_completion() -> Result {
        with_exporter(|fixture| {
            let _connector = fixture.drm.publish_connector_identity()?;
            let creator = fixture.drm.master_file()?;
            let _fb = select(fixture, &creator)?;
            let grantor = grant(fixture, &creator)?;
            with_client(grantor.capture(), |file| {
                let first = destination(fixture, Layout::new(640, 480)?)?;
                let replacement = destination(fixture, Layout::new(640, 480)?)?;
                let mut registered = register(file, 1, &first)?;
                let offer = Description::query(file)?.id();
                let mut stream = ClientStream::open(file, 1, offer, 1)?;
                let readiness = Readiness::for_client(file)?;
                check(stream.queue_output(1, 7, None) == Err(ENOENT))?;
                let mut reuse = ManualFence::new()?;
                stream.queue_output(1, 1, Some(&reuse.fence()))?;
                registered.unregister()?;
                let _replacement = register(file, 2, &replacement)?;
                reuse.complete(Ok(()))?;
                wait(&readiness)?;
                check(stream.cancel(1) == Err(EALREADY))?;
                check(stream.dequeue::<()>(|_| Err(EFAULT)) == Err(EFAULT))?;
                check(readiness.has_results())?;
                stream.dequeue(|completion| {
                    check(completion.use_id() == 1)?;
                    completion.result().map(|_| ())
                })?;
                check(!readiness.has_results())?;
                check(stream.cancel(1) == Err(ENOENT))?;
                check(
                    pixels(first.buffer())?[128..160]
                        .chunks_exact(4)
                        .all(|pixel| pixel == [0x12, 0x12, 0x12, 0xff]),
                )?;
                check(
                    pixels(replacement.buffer())?
                        .iter()
                        .all(|byte| *byte == 0x73),
                )?;
                check(stream.queue_output(2, 1, None) == Err(ENOENT))?;
                stream.close()
            })
        })
    }

    #[test]
    fn file_cancellation_does_not_wait_for_pending_destination_reuse() -> Result {
        with_exporter(|fixture| {
            let _connector = fixture.drm.publish_connector_identity()?;
            let creator = fixture.drm.master_file()?;
            let _fb = select(fixture, &creator)?;
            let grantor = grant(fixture, &creator)?;
            with_client(grantor.capture(), |file| {
                let image = destination(fixture, Layout::new(640, 480)?)?;
                let _registered = register(file, 1, &image)?;
                let mut stream = ClientStream::open(file, 1, Description::query(file)?.id(), 1)?;
                let readiness = Readiness::for_client(file)?;
                let reuse = ManualFence::new()?;
                stream.queue_output(1, 1, Some(&reuse.fence()))?;
                check(stream.cancel(2) == Err(ENOENT))?;
                stream.cancel(1)?;
                check(stream.cancel(1) == Err(EALREADY))?;
                wait(&readiness)?;
                check(stream.dequeue::<()>(|_| Err(EFAULT)) == Err(EFAULT))?;
                stream.dequeue(|completion| {
                    check(completion.use_id() == 1)?;
                    check(matches!(completion.result(), Err(ECANCELED)))
                })?;
                check(reuse.fence().status() == kernel::dma_fence::Status::Pending)?;
                check(pixels(image.buffer())?.iter().all(|byte| *byte == 0x73))?;
                stream.close()
            })
        })
    }

    #[test]
    fn revoked_file_keeps_terminal_errors_and_cleanup() -> Result {
        with_exporter(|fixture| {
            let _connector = fixture.drm.publish_connector_identity()?;
            let creator = fixture.drm.master_file()?;
            let _fb = select(fixture, &creator)?;
            let grantor = grant(fixture, &creator)?;
            with_client(grantor.capture(), |file| {
                let image = destination(fixture, Layout::new(640, 480)?)?;
                let mut registered = register(file, 1, &image)?;
                let mut stream = ClientStream::open(file, 1, Description::query(file)?.id(), 1)?;
                let readiness = Readiness::for_client(file)?;
                let mut reuse = ManualFence::new()?;
                reuse.complete(Err(EIO))?;
                stream.queue_output(1, 1, Some(&reuse.fence()))?;
                wait(&readiness)?;
                drop(grantor);
                check(stream.queue_output(2, 1, None) == Err(EKEYREVOKED))?;
                stream.dequeue(|completion| {
                    check(completion.use_id() == 1)?;
                    check(matches!(completion.result(), Err(EIO)))
                })?;
                check(pixels(image.buffer())?.iter().all(|byte| *byte == 0x73))?;
                stream.close()?;
                registered.unregister()
            })
        })
    }
}
