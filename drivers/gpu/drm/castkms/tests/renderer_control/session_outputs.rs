// SPDX-License-Identifier: GPL-2.0-only

use super::{
    private_images::{buffer, profile},
    *,
};
use crate::{
    capture::provider::{delegated_destination::Image, Grantor},
    renderer::{job::Completion, output_broker::Registration, session::Session},
};
use kernel::{
    dma_buf::DmaBuf,
    dma_fence::testing::ManualFence,
    drm::{fourcc, gem::ExportAccess, preparation::Source},
    sync::aref::ARef,
    time::{delay::fsleep, Delta, Instant, Monotonic},
};

struct Fixture {
    _owner: Owner,
    _grantor: Grantor,
    session: Arc<Session>,
    registration: Registration,
    destination: Arc<Image>,
    private: ARef<DmaBuf>,
    source: ARef<Source>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.session.close_for_test();
    }
}

fn with_session(f: impl FnOnce(Fixture) -> Result) -> Result {
    with_display(|device, crtc, connector, _, file| {
        let owner = owner(&file, crtc, connector)?;
        let session = Session::new(owner.access(), device.to_registered_ref())?;
        let pending = session.begin(device.execution.describe().generation)?;
        let candidate = pending.id();
        pending.publish()?;
        session.candidate(candidate)?.submit_private_probe(None)?;
        let proposal = session.propose_profile(candidate, profile()?)?;
        device.atomic_update(|transaction| {
            transaction
                .add_crtc_state(crtc)?
                .tag_transition(proposal.transition);
            Ok(())
        })?;
        session.activate(candidate)?;
        let grantor = super::delegated_authority::grant(&file, crtc, connector)?;
        let scope = grantor.capture().describe_delegated()?;
        let registration = scope.register_routed_queue(1)?;
        let destination = scope.register_destination(
            &buffer(device, ExportAccess::ReadWrite)?,
            fourcc::XRGB8888,
            0,
            2560,
            0,
        )?;
        let private = buffer(device, ExportAccess::ReadWrite)?;
        session.register_image(1, [640, 480], core::slice::from_ref(&private))?;
        let pending = session.begin_source(1)?;
        let id = pending.id();
        pending.publish(|| {})?;
        session.release_source(id, Completion::Cpu)?;
        let source = device
            .output
            .with_accepted(|accepted| accepted.map(|accepted| ARef::from(accepted.source)))
            .ok_or(EINVAL)?;
        f(Fixture {
            _owner: owner,
            _grantor: grantor,
            session,
            registration,
            destination,
            private,
            source,
        })
    })
}

fn wait_result(registration: &Registration, expected: Result) -> Result {
    let start = Instant::<Monotonic>::now();
    loop {
        if registration.with_queue(|queue| {
            queue.advance();
            Ok(queue.has_results())
        })? {
            return registration
                .with_queue(|queue| queue.dequeue(|result| check(result.result == expected)));
        }
        if start.elapsed() > Delta::from_secs(2) {
            return Err(ETIMEDOUT);
        }
        fsleep(Delta::from_millis(1));
    }
}

#[kunit_tests(rust_castkms_session_outputs)]
mod cases {
    use super::*;

    #[test]
    fn publication_rollback_preserves_the_private_image() -> Result {
        with_session(|fixture| {
            fixture
                .registration
                .with_queue(|queue| queue.queue_to(1, &fixture.destination, None))?;
            let pending = fixture.session.begin_output(1)?;
            check(pending.image_id() == 1)?;
            check(core::ptr::eq(pending.destination()?, &*fixture.destination))?;
            check(fixture.session.begin_output(1).err() == Some(EBUSY))?;
            let first_id = pending.id();
            drop(pending);
            wait_result(&fixture.registration, Err(ECANCELED))?;
            check(fixture.session.begin_output(1).err() == Some(ENODATA))?;
            fixture
                .registration
                .with_queue(|queue| queue.queue_to(2, &fixture.destination, None))?;
            let pending = fixture.session.begin_output(1)?;
            let id = pending.id();
            check(id > first_id)?;
            pending.publish(|| {})?;
            check(fixture.session.begin_output(1).err() == Some(EBUSY))?;
            check(
                fixture
                    .session
                    .release_output(id + 1, Completion::WithoutAccess)
                    == Err(ENOENT),
            )?;
            fixture.session.release_output(id, Completion::Cpu)?;
            fixture
                .session
                .release_output(id, Completion::WithoutAccess)?;
            wait_result(&fixture.registration, Ok(()))
        })
    }

    #[test]
    fn closing_an_unpublished_output_installs_nothing() -> Result {
        with_session(|fixture| {
            fixture
                .registration
                .with_queue(|queue| queue.queue_to(1, &fixture.destination, None))?;
            let pending = fixture.session.begin_output(1)?;
            fixture.session.close_for_test();
            let mut publications = 0;
            check(pending.publish(|| publications += 1) == Err(EKEYREVOKED))?;
            check(publications == 0)?;
            wait_result(&fixture.registration, Err(ECANCELED))
        })
    }

    #[test]
    fn native_output_holds_private_storage_but_not_source_reads() -> Result {
        with_session(|fixture| {
            fixture
                .registration
                .with_queue(|queue| queue.queue_to(1, &fixture.destination, None))?;
            let pending = fixture.session.begin_output(1)?;
            let id = pending.id();
            pending.publish(|| {})?;
            let mut native = ManualFence::new()?;
            fixture
                .session
                .release_output(id, Completion::Submitted(native.fence()))?;
            fixture.source.seal();
            check(
                fixture
                    .source
                    .prepared()?
                    .ok_or(EINVAL)?
                    .completion()?
                    .is_none(),
            )?;
            fixture.session.unregister_image(1)?;
            check(
                fixture.session.register_image(
                    2,
                    [640, 480],
                    core::slice::from_ref(&fixture.private),
                ) == Err(EEXIST),
            )?;
            check(fixture.destination.reserve(2, None).err() == Some(EBUSY))?;
            native.complete(Ok(()))?;
            wait_result(&fixture.registration, Ok(()))?;
            let start = Instant::<Monotonic>::now();
            loop {
                match fixture.session.register_image(
                    2,
                    [640, 480],
                    core::slice::from_ref(&fixture.private),
                ) {
                    Ok(()) => break,
                    Err(EEXIST) if start.elapsed() < Delta::from_secs(2) => {
                        fsleep(Delta::from_millis(1))
                    }
                    Err(error) => return Err(error),
                }
            }
            fixture.session.unregister_image(2)
        })
    }
}
