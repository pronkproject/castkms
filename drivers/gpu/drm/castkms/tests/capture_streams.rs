// SPDX-License-Identifier: GPL-2.0-only

//! Stream lifetime mechanics, with test-owned streams and synthetic control transitions.

use super::*;
use crate::capture::streams::Registry;
use kernel::{
    drm::capture::{
        Status,
        Stream, //
    },
    sync::{
        Arc,
        Completion, //
    },
    workqueue::{
        self,
        impl_has_work,
        new_work,
        Work,
        WorkItem, //
    }, //
};

#[pin_data]
struct Revoker {
    #[pin]
    work: Work<Self>,
    #[pin]
    ready: Completion,
    #[pin]
    proceed: Completion,
    registry: Arc<Registry>,
}

impl_has_work! {
    impl HasWork<Self> for Revoker { self.work }
}

impl WorkItem for Revoker {
    type Pointer = Arc<Self>;

    fn run(worker: Arc<Self>) {
        worker.ready.complete_all();
        worker.proceed.wait_for_completion();
        worker.registry.revoke_all();
    }
}

struct Revocation(Arc<Revoker>);

impl Revocation {
    fn start(registry: Arc<Registry>) -> Result<Self> {
        let owner = Self(Arc::pin_init(
            pin_init!(Revoker {
                work <- new_work!("CastKMS test stream revocation"),
                ready <- Completion::new(),
                proceed <- Completion::new(),
                registry,
            }),
            GFP_KERNEL,
        )?);
        check(workqueue::system_dfl().enqueue(owner.0.clone()).is_ok())?;
        owner.0.ready.wait_for_completion();
        Ok(owner)
    }
}

impl Drop for Revocation {
    fn drop(&mut self) {
        self.0.proceed.complete_all();
        self.0.work.flush();
    }
}

#[kunit_tests(rust_castkms_capture_streams)]
mod cases {
    use super::*;

    #[test]
    fn control_change_revokes_queued_work() -> Result {
        let registry = Registry::new()?;
        let stream = Stream::new(1, 4)?;
        let _registration = registry.register(&stream)?;
        let request = stream.queue()?;
        registry.revoke_all();
        check(request.status()? == Status::Complete(Err(EKEYREVOKED)))?;
        check(matches!(stream.queue(), Err(EKEYREVOKED)))?;
        Ok(())
    }

    #[test]
    fn claimed_storage_survives_without_publishing_success() -> Result {
        let registry = Registry::new()?;
        let stream = Stream::new(1, 4)?;
        let _registration = registry.register(&stream)?;
        let request = stream.queue()?;
        let mut job = stream.claim()?;
        registry.revoke_all();
        check(request.status()? == Status::Pending)?;
        job.data_mut().copy_from_slice(&[0x35; 4]);
        job.complete(Ok(()));
        check(request.status()? == Status::Complete(Err(EKEYREVOKED)))?;
        let mut pixels = [0xa7; 4];
        check(request.copy_result(&mut pixels) == Err(EKEYREVOKED))?;
        check(pixels == [0xa7; 4])?;
        Ok(())
    }

    #[test]
    fn completed_results_keep_their_authorized_old_pixels() -> Result {
        let registry = Registry::new()?;
        let stream = Stream::new(1, 4)?;
        let registration = registry.register(&stream)?;
        let request = stream.queue()?;
        let mut job = stream.claim()?;
        job.data_mut().copy_from_slice(&[0x35; 4]);
        job.complete(Ok(()));
        registry.revoke_all();
        registry.close();
        drop(registration);
        let mut pixels = [0; 4];
        check(request.copy_result(&mut pixels)? == 4)?;
        check(pixels == [0x35; 4])?;
        Ok(())
    }

    #[test]
    fn new_control_does_not_revive_an_old_stream() -> Result {
        let registry = Registry::new()?;
        let old = Stream::new(1, 4)?;
        let registration = registry.register(&old)?;
        registry.revoke_all();
        let new = Stream::new(1, 4)?;
        let _replacement = registry.register(&new)?;
        drop(registration);
        check(matches!(old.queue(), Err(EKEYREVOKED)))?;
        let request = new.queue()?;
        new.claim()?.complete(Ok(()));
        check(request.status()? == Status::Complete(Ok(())))?;
        Ok(())
    }

    #[test]
    fn closing_one_registration_preserves_its_sibling() -> Result {
        let registry = Registry::new()?;
        let first = Stream::new(1, 4)?;
        let second = Stream::new(1, 4)?;
        let registration = registry.register(&first)?;
        let _sibling = registry.register(&second)?;
        check(matches!(registry.register(&first), Err(EEXIST)))?;
        let _first_request = first.queue()?;
        drop(registration);
        check(matches!(first.queue(), Err(EKEYREVOKED)))?;
        let request = second.queue()?;
        second.claim()?.complete(Ok(()));
        check(request.status()? == Status::Complete(Ok(())))?;
        Ok(())
    }

    #[test]
    fn permanent_close_rejects_new_registrations() -> Result {
        let registry = Registry::new()?;
        let stream = Stream::new(1, 4)?;
        let registration = registry.register(&stream)?;
        registry.close();
        registry.revoke_all();
        registry.close();
        let replacement = Stream::new(1, 4)?;
        check(matches!(registry.register(&replacement), Err(ENODEV)))?;
        check(matches!(stream.queue(), Err(EKEYREVOKED)))?;
        drop(registry);
        drop(registration);
        Ok(())
    }

    #[test]
    fn old_handles_cannot_remove_a_new_registration_of_the_same_stream() -> Result {
        let registry = Registry::new()?;
        let stream = Stream::new(1, 4)?;
        let old = registry.register(&stream)?;
        registry.revoke_all();
        let _new = registry.register(&stream)?;
        drop(old);
        check(matches!(registry.register(&stream), Err(EEXIST)))?;
        // Tracking a revoked stream cannot restore its native admission.
        check(matches!(stream.queue(), Err(EKEYREVOKED)))?;
        Ok(())
    }

    #[test]
    fn device_callbacks_revoke_streams_and_shutdown_closes_admission() -> Result {
        let fixture = Fixture::new()?;
        let registry = fixture.drm.device().capture_streams.clone();
        let old = Stream::new(1, 4)?;
        let _old_registration = registry.register(&old)?;
        <Driver as drm::Driver>::master_changed(fixture.drm.device(), None);
        check(matches!(old.queue(), Err(EKEYREVOKED)))?;
        let new = Stream::new(1, 4)?;
        let _new_registration = registry.register(&new)?;
        fixture.state.close();
        check(matches!(new.queue(), Err(EKEYREVOKED)))?;
        check(matches!(registry.register(&new), Err(ENODEV)))?;
        Ok(())
    }

    #[test]
    fn removing_a_stream_can_race_control_revocation() -> Result {
        for _ in 0..64 {
            let registry = Registry::new()?;
            let stream = Stream::new(1, 4)?;
            let registration = registry.register(&stream)?;
            let request = stream.queue()?;
            let worker = Revocation::start(registry.clone())?;
            worker.0.proceed.complete_all();
            drop(registration);
            registry.close();
            drop(worker);
            check(request.status()? == Status::Complete(Err(EKEYREVOKED)))?;
            check(matches!(stream.queue(), Err(EKEYREVOKED)))?;
        }
        Ok(())
    }
}
