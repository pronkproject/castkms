// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use crate::{capture::request_budget::QUEUE_LIMIT, renderer::output_broker::Broker};

fn register(
    broker: &Arc<Broker>,
    fixture: &Fixture,
) -> Result<crate::renderer::output_broker::Registration> {
    broker.register(|| {
        fixture
            .grantor
            .capture()
            .describe_delegated()?
            .create_queue(&fixture.renderer, &fixture.active, 1)
    })
}

fn destination(fixture: &Fixture) -> Result<Arc<Destination>> {
    fixture
        .grantor
        .capture()
        .describe_delegated()?
        .register_destination(
            &buffer(fixture.owner.access().device(), ExportAccess::ReadWrite)?,
            fourcc::XRGB8888,
            0,
            2560,
            0,
        )
}

#[kunit_tests(rust_castkms_output_broker)]
mod cases {
    use super::*;

    #[test]
    fn revoked_recipient_does_not_block_an_independent_grant() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let fixture = output_fixture(device, crtc, connector, &file)?;
            let broker = Broker::new()?;
            let revoked = register(&broker, &fixture)?;
            let independent = grant(&file, crtc, connector)?;
            let scope = independent.capture().describe_delegated()?;
            let destination = scope.register_destination(
                &buffer(device, ExportAccess::ReadWrite)?,
                fourcc::XRGB8888,
                0,
                2560,
                0,
            )?;
            let ready = broker.register(|| {
                scope.create_queue(&fixture.renderer, &fixture.active, 1)
            })?;
            revoked.with_queue(|queue| queue.queue_to(1, &fixture.destination, None))?;
            ready.with_queue(|queue| queue.queue_to(1, &destination, None))?;
            drop(fixture.grantor);
            let job = broker.try_claim(&fixture.rendered).ok_or(EINVAL)?;
            check(job.queue_id == ready.id())?;
            check(core::ptr::eq(job.output.claim.destination(), &*destination))?;
            job.output.claim.release(Completion::Cpu);
            revoked.with_queue(|queue| {
                queue.advance();
                queue.dequeue(|result| check(result.result == Err(EKEYREVOKED)))
            })?;
            ready.with_queue(|queue| {
                queue.advance();
                queue.dequeue(|result| check(result.result == Ok(())))
            })
        })
    }

    #[test]
    fn registrations_are_bounded_and_names_are_not_reused() -> Result {
        with_output(|fixture| {
            let broker = Broker::new()?;
            let mut registrations = KVec::new();
            for index in 0..QUEUE_LIMIT {
                let registration = register(&broker, &fixture)?;
                check(registration.id() == index as u64 + 1)?;
                registrations.push(registration, GFP_KERNEL)?;
            }
            let mut calls = 0;
            check(
                broker
                    .register(|| {
                        calls += 1;
                        Err(EIO)
                    })
                    .err()
                    == Some(EBUSY),
            )?;
            check(calls == 0)?;
            drop(registrations.pop());
            check(register(&broker, &fixture)?.id() == QUEUE_LIMIT as u64 + 1)?;
            drop(registrations);
            check(register(&broker, &fixture)?.id() == QUEUE_LIMIT as u64 + 2)
        })
    }

    #[test]
    fn allocation_callback_runs_outside_directory_exclusion() -> Result {
        with_output(|fixture| {
            let broker = Broker::new()?;
            let result = broker.register(|| {
                broker.close();
                fixture
                    .grantor
                    .capture()
                    .describe_delegated()?
                    .create_queue(&fixture.renderer, &fixture.active, 1)
            });
            check(result.err() == Some(ESHUTDOWN))?;
            let mut calls = 0;
            check(
                broker
                    .register(|| {
                        calls += 1;
                        Err(EIO)
                    })
                    .err()
                    == Some(ESHUTDOWN),
            )?;
            check(calls == 0)
        })
    }

    #[test]
    fn ready_recipients_are_selected_round_robin() -> Result {
        with_output(|fixture| {
            let broker = Broker::new()?;
            let first = register(&broker, &fixture)?;
            let second = register(&broker, &fixture)?;
            let third = register(&broker, &fixture)?;
            let images = [
                destination(&fixture)?,
                destination(&fixture)?,
                destination(&fixture)?,
            ];
            for (registration, image) in [&first, &second, &third].into_iter().zip(&images) {
                registration.with_queue(|queue| queue.queue_to(1, image, None))?;
            }
            let job = broker.try_claim(&fixture.rendered).ok_or(EINVAL)?;
            check(job.queue_id == first.id() && job.output.use_id == 1)?;
            job.output.claim.release(Completion::Cpu);
            first.with_queue(|queue| {
                queue.advance();
                queue.dequeue(|result| check(result.result == Ok(())))?;
                queue.queue_to(2, &images[0], None)
            })?;
            for expected in [second.id(), third.id(), first.id()] {
                let job = broker.try_claim(&fixture.rendered).ok_or(EINVAL)?;
                check(job.queue_id == expected)?;
                job.output.claim.release(Completion::Cpu);
            }
            check(broker.try_claim(&fixture.rendered).is_none())
        })
    }

    #[test]
    fn pending_recipient_reuse_does_not_hide_ready_demand() -> Result {
        with_output(|fixture| {
            let broker = Broker::new()?;
            let blocked = register(&broker, &fixture)?;
            let ready = register(&broker, &fixture)?;
            let mut reuse = ManualFence::new()?;
            blocked
                .with_queue(|queue| queue.queue_to(1, &fixture.destination, Some(reuse.fence())))?;
            ready.with_queue(|queue| queue.queue_to(1, &destination(&fixture)?, None))?;
            let job = broker.try_claim(&fixture.rendered).ok_or(EINVAL)?;
            check(job.queue_id == ready.id())?;
            job.output.claim.release(Completion::WithoutAccess);
            check(broker.try_claim(&fixture.rendered).is_none())?;
            reuse.complete(Ok(()))?;
            let job = broker.try_claim(&fixture.rendered).ok_or(EINVAL)?;
            check(job.queue_id == blocked.id())?;
            job.output.claim.release(Completion::WithoutAccess);
            Ok(())
        })
    }

    #[test]
    fn closing_discovery_preserves_pending_native_access() -> Result {
        with_output(|fixture| {
            let broker = Broker::new()?;
            let registration = register(&broker, &fixture)?;
            registration.with_queue(|queue| queue.queue_to(1, &fixture.destination, None))?;
            let job = broker.try_claim(&fixture.rendered).ok_or(EINVAL)?;
            let mut native = ManualFence::new()?;
            job.output
                .claim
                .release(Completion::Submitted(native.fence()));
            broker.close();
            check(broker.try_claim(&fixture.rendered).is_none())?;
            registration.with_queue(|queue| {
                check(queue.try_close() == Err(EBUSY))?;
                check(queue.advance() == 0)
            })?;
            check(fixture.destination.reserve(2, None).err() == Some(EBUSY))?;
            native.complete(Ok(()))?;
            let start = Instant::<Monotonic>::now();
            loop {
                let ready = registration.with_queue(|queue| {
                    queue.advance();
                    Ok(queue.has_results())
                })?;
                if ready {
                    break;
                }
                if start.elapsed() > Delta::from_secs(2) {
                    return Err(ETIMEDOUT);
                }
                fsleep(Delta::from_millis(1));
            }
            registration.with_queue(|queue| {
                queue.dequeue(|result| {
                    check(result.result == Err(ECANCELED) && result.native.is_some())
                })
            })
        })
    }
}
