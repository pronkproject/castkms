// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use crate::capture::provider::delegated_queue::Queue;

fn queue(fixture: &Fixture, capacity: u32) -> Result<Queue> {
    fixture
        .grantor
        .capture()
        .describe_delegated()?
        .create_queue(&fixture.renderer, &fixture.active, capacity)
}

fn wait_ready(queue: &mut Queue) -> Result {
    let start = Instant::<Monotonic>::now();
    loop {
        queue.advance();
        if queue.has_results() {
            return Ok(());
        }
        if start.elapsed() > Delta::from_secs(2) {
            return Err(ETIMEDOUT);
        }
        fsleep(Delta::from_millis(1));
    }
}

#[kunit_tests(rust_castkms_delegated_queue)]
mod cases {
    use super::*;
    use kernel::sync::poll::testing::Observer;

    #[test]
    fn lost_native_access_is_not_published_as_completed_output() -> Result {
        with_output(|fixture| {
            let mut queue = queue(&fixture, 1)?;
            queue.queue_to(1, &fixture.destination, None)?;
            let job = queue.try_claim(&fixture.rendered).ok_or(EINVAL)?;
            // Exercise the actual unreported-access path. Its bounded E/D retention is
            // intentionally quarantined, not freed by the fixture or an invented fence.
            drop(job);
            check(queue.advance() == 1)?;
            let mut publications = 0;
            for _ in 0..2 {
                check(queue.dequeue(|_| { publications += 1; Ok(()) }) == Err(EIO))?;
                check(queue.has_results())?;
            }
            check(publications == 0)?;
            check(queue.try_close() == Err(EIO))?;
            drop(queue);
            check(fixture.destination.reserve(2, None).err() == Some(EBUSY))?;
            drop(fixture.rendered);
            check(fixture.private.prepare(2).err() == Some(EBUSY))
        })
    }

    #[test]
    fn observed_queue_admission_does_not_extend_worker_ownership() -> Result {
        with_output(|fixture| {
            let scope = fixture.grantor.capture().describe_delegated()?;
            let observation = fixture.active.observation();
            let mut queue = scope.create_queue_observed(&fixture.renderer, &observation, 1)?;
            queue.queue_to(1, &fixture.destination, None)?;
            drop(fixture.active);
            check(
                scope.create_queue_observed(&fixture.renderer, &observation, 1).err()
                    == Some(EIO),
            )?;
            check(queue.advance() == 1)?;
            queue.dequeue(|result| check(result.result == Err(EIO)))
        })
    }

    fn wait_notification(observer: &Observer, before: usize) -> Result {
        let start = Instant::<Monotonic>::now();
        while observer.notifications() == before {
            if start.elapsed() > Delta::from_secs(2) {
                return Err(ETIMEDOUT);
            }
            fsleep(Delta::from_millis(1));
        }
        Ok(())
    }

    #[test]
    fn destination_reuse_and_output_retirement_wake_waiters() -> Result {
        with_output(|fixture| {
            let mut queue = queue(&fixture, 1)?;
            let observer = Observer::new(queue.changed().clone())?;
            let mut reuse = ManualFence::new()?;
            queue.queue_to(1, &fixture.destination, Some(reuse.fence()))?;
            check(observer.notifications() > 0)?;
            check(queue.try_claim(&fixture.rendered).is_none())?;
            let before = observer.notifications();
            reuse.complete(Ok(()))?;
            check(observer.notifications() > before)?;
            let job = queue.try_claim(&fixture.rendered).ok_or(EINVAL)?;
            let mut completion = ManualFence::new()?;
            job.claim.release(Completion::Submitted(completion.fence()));
            let before = observer.notifications();
            completion.complete(Ok(()))?;
            wait_ready(&mut queue)?;
            wait_notification(&observer, before)?;
            queue.dequeue(|result| check(result.result == Ok(())))
        })
    }

    #[test]
    fn queued_cancellation_detaches_reuse_notification() -> Result {
        with_output(|fixture| {
            let mut queue = queue(&fixture, 1)?;
            let observer = Observer::new(queue.changed().clone())?;
            let mut reuse = ManualFence::new()?;
            queue.queue_to(1, &fixture.destination, Some(reuse.fence()))?;
            let before = observer.notifications();
            queue.cancel(1)?;
            check(observer.notifications() > before)?;
            let before = observer.notifications();
            reuse.complete(Ok(()))?;
            check(observer.notifications() == before)?;
            queue.advance();
            queue.dequeue(|result| check(result.result == Err(ECANCELED)))
        })
    }

    #[test]
    fn authority_and_worker_loss_wake_unclaimed_demand() -> Result {
        for reason in 0..3 {
            with_output(|fixture| {
                let mut queue = queue(&fixture, 1)?;
                queue.queue_to(1, &fixture.destination, None)?;
                let observer = Observer::new(queue.changed().clone())?;
                let expected = match reason {
                    0 => {
                        drop(fixture.active);
                        EIO
                    }
                    1 => {
                        drop(fixture.grantor);
                        EKEYREVOKED
                    }
                    _ => {
                        fixture.owner.revoke();
                        EKEYREVOKED
                    }
                };
                check(observer.notifications() > 0)?;
                check(queue.advance() == 1)?;
                queue.dequeue(|result| check(result.result == Err(expected)))
            })?;
        }
        Ok(())
    }

    #[test]
    fn disabled_output_wakes_and_terminates_queued_demand() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let fixture = output_fixture(device, crtc, connector, &file)?;
            let mut queue = queue(&fixture, 1)?;
            queue.queue_to(1, &fixture.destination, None)?;
            let observer = Observer::new(queue.changed().clone())?;
            device.atomic_update(|transaction| transaction.set_crtc_config(crtc, None))?;
            check(observer.notifications() > 0)?;
            check(queue.advance() == 1)?;
            queue.dequeue(|result| check(result.result == Err(ENODEV)))
        })
    }

    #[test]
    fn master_close_wakes_queued_demand() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let fixture = output_fixture(device, crtc, connector, &file)?;
            let mut queue = queue(&fixture, 1)?;
            queue.queue_to(1, &fixture.destination, None)?;
            let observer = Observer::new(queue.changed().clone())?;
            drop(file);
            check(observer.notifications() > 0)?;
            check(queue.advance() == 1)?;
            queue.dequeue(|result| check(result.result == Err(EACCES)))
        })
    }

    #[test]
    fn failed_reuse_wakes_a_terminal_result_without_a_private_image() -> Result {
        with_output(|fixture| {
            let mut queue = queue(&fixture, 1)?;
            let mut reuse = ManualFence::new()?;
            queue.queue_to(1, &fixture.destination, Some(reuse.fence()))?;
            drop(fixture.rendered);
            let observer = Observer::new(queue.changed().clone())?;
            reuse.complete(Err(EAGAIN))?;
            check(observer.notifications() > 0)?;
            check(queue.advance() == 1)?;
            queue.dequeue(|result| check(result.result == Err(EAGAIN)))
        })
    }

    #[test]
    fn private_write_completion_wakes_waiting_output_claims() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let fixture = output_fixture(device, crtc, connector, &file)?;
            let mut queue = queue(&fixture, 1)?;
            let private = fixture.renderer.register_private_image(
                &fixture.active,
                [640, 480],
                &[buffer(device, ExportAccess::ReadWrite)?],
            )?;
            let mut completion = ManualFence::new()?;
            let rendered = Arc::new(
                fixture
                    .renderer
                    .claim_render(
                        &fixture.active,
                        fixture.execution,
                        None,
                        private.prepare(1)?,
                    )?
                    .release(Completion::Submitted(completion.fence()))
                    .ok_or(EINVAL)?,
                GFP_KERNEL,
            )?;
            queue.queue_to(1, &fixture.destination, None)?;
            check(queue.try_claim(&rendered).is_none())?;
            let observer = Observer::new(queue.changed().clone())?;
            completion.complete(Ok(()))?;
            wait_notification(&observer, 0)?;
            let job = queue.try_claim(&rendered).ok_or(EINVAL)?;
            job.claim.release(Completion::Cpu);
            let before = observer.notifications();
            drop(rendered);
            wait_notification(&observer, before)?;
            drop(private.prepare(2)?);
            Ok(())
        })
    }

    #[test]
    fn failed_publication_preserves_terminal_credit_and_identity() -> Result {
        with_output(|fixture| {
            check(queue(&fixture, 0).err() == Some(EINVAL))?;
            check(queue(&fixture, 9).err() == Some(E2BIG))?;
            let mut queue = queue(&fixture, 1)?;
            queue.queue_to(1, &fixture.destination, None)?;
            check(queue.has_pending() && !queue.has_results())?;
            check(queue.queue_to(2, &fixture.destination, None) == Err(EAGAIN))?;
            let job = queue.try_claim(&fixture.rendered).ok_or(EINVAL)?;
            let id = job.use_id;
            job.claim.release(Completion::Cpu);
            check(id == 1)?;
            check(queue.advance() == 1)?;
            check(!queue.has_pending() && queue.has_results())?;
            check(queue.dequeue::<()>(|_| Err(EFAULT)) == Err(EFAULT))?;
            check(queue.queue_to(2, &fixture.destination, None) == Err(EAGAIN))?;
            queue.dequeue(|result| {
                check(result.use_id == 1 && result.result == Ok(()))?;
                check(result.content == fixture.rendered.content().content_serial())?;
                check(result.native.is_none())
            })?;
            check(queue.queue_to(1, &fixture.destination, None) == Err(ESTALE))?;
            queue.queue_to(2, &fixture.destination, None)?;
            queue.cancel(2)?;
            check(queue.advance() == 1)?;
            queue.dequeue(|result| check(result.result == Err(ECANCELED)))?;
            check(queue.dequeue(|_| Ok(())) == Err(EAGAIN))?;
            queue.try_close()
        })
    }

    #[test]
    fn blocked_destination_does_not_hide_a_later_ready_use() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let fixture = output_fixture(device, crtc, connector, &file)?;
            let second = fixture
                .grantor
                .capture()
                .describe_delegated()?
                .register_destination(
                    &buffer(device, ExportAccess::ReadWrite)?,
                    fourcc::XRGB8888,
                    0,
                    2560,
                    0,
                )?;
            let mut queue = queue(&fixture, 2)?;
            let mut reuse = ManualFence::new()?;
            queue.queue_to(1, &fixture.destination, Some(reuse.fence()))?;
            queue.queue_to(2, &second, None)?;
            let job = queue.try_claim(&fixture.rendered).ok_or(EINVAL)?;
            let id = job.use_id;
            job.claim.release(Completion::Cpu);
            check(id == 2)?;
            check(queue.advance() == 1)?;
            queue.dequeue(|result| check(result.use_id == 2 && result.result == Ok(())))?;
            check(queue.has_pending())?;
            reuse.complete(Ok(()))?;
            let job = queue.try_claim(&fixture.rendered).ok_or(EINVAL)?;
            let id = job.use_id;
            job.claim.release(Completion::Cpu);
            check(id == 1)?;
            check(queue.advance() == 1)?;
            queue.dequeue(|result| check(result.use_id == 1 && result.result == Ok(())))
        })
    }

    #[test]
    fn same_output_does_not_make_another_grants_storage_compatible() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let fixture = output_fixture(device, crtc, connector, &file)?;
            let foreign = grant(&file, crtc, connector)?;
            let destination = foreign
                .capture()
                .describe_delegated()?
                .register_destination(
                    &buffer(device, ExportAccess::ReadWrite)?,
                    fourcc::XRGB8888,
                    0,
                    2560,
                    0,
                )?;
            let mut queue = queue(&fixture, 1)?;
            check(queue.queue_to(1, &destination, None) == Err(EACCES))?;
            queue.queue_to(1, &fixture.destination, None)?;
            queue.try_close()
        })
    }

    #[test]
    fn failed_reuse_is_a_terminal_record_not_empty_dequeue() -> Result {
        with_output(|fixture| {
            let mut queue = queue(&fixture, 1)?;
            let mut reuse = ManualFence::new()?;
            queue.queue_to(1, &fixture.destination, Some(reuse.fence()))?;
            reuse.complete(Err(EAGAIN))?;
            check(queue.try_claim(&fixture.rendered).is_none())?;
            check(queue.has_results() && !queue.has_pending())?;
            queue.dequeue(|result| {
                check(result.result == Err(EAGAIN))?;
                check(result.content.is_none() && result.native.is_none())
            })
        })
    }

    #[test]
    fn revoked_queued_demand_does_not_wait_for_destination_reuse() -> Result {
        with_output(|fixture| {
            let mut queue = queue(&fixture, 1)?;
            let reuse = ManualFence::new()?;
            queue.queue_to(1, &fixture.destination, Some(reuse.fence()))?;
            drop(fixture.grantor);
            check(queue.advance() == 1)?;
            queue.dequeue(|result| check(result.result == Err(EKEYREVOKED)))?;
            queue.try_close()
        })
    }

    #[test]
    fn queue_does_not_keep_its_worker_active() -> Result {
        with_output(|fixture| {
            let mut queue = queue(&fixture, 1)?;
            queue.queue_to(1, &fixture.destination, None)?;
            drop(fixture.active);
            check(queue.advance() == 1)?;
            queue.dequeue(|result| check(result.result == Err(EIO)))
        })
    }

    #[test]
    fn successful_completion_is_not_publication_after_revocation() -> Result {
        with_output(|fixture| {
            let mut queue = queue(&fixture, 1)?;
            queue.queue_to(1, &fixture.destination, None)?;
            queue
                .try_claim(&fixture.rendered)
                .ok_or(EINVAL)?
                .claim
                .release(Completion::Cpu);
            check(queue.advance() == 1)?;
            drop(fixture.grantor);
            queue.dequeue(|result| {
                check(result.result == Err(EKEYREVOKED))?;
                check(result.content.is_none())
            })
        })
    }

    #[test]
    fn cancellation_keeps_native_cleanup_evidence_until_acknowledged() -> Result {
        with_output(|fixture| {
            let mut queue = queue(&fixture, 1)?;
            queue.queue_to(1, &fixture.destination, None)?;
            let mut native = ManualFence::new()?;
            queue
                .try_claim(&fixture.rendered)
                .ok_or(EINVAL)?
                .claim
                .release(Completion::Submitted(native.fence()));
            queue.cancel(1)?;
            check(queue.try_close() == Err(EBUSY))?;
            check(queue.queue_to(2, &fixture.destination, None) == Err(ESHUTDOWN))?;
            native.complete(Ok(()))?;
            wait_ready(&mut queue)?;
            queue.dequeue(|result| {
                check(result.result == Err(ECANCELED))?;
                check(result.content.is_none())?;
                check(
                    result.native.ok_or(EINVAL)?.status()
                        == kernel::dma_fence::Status::Complete(Ok(())),
                )
            })?;
            queue.try_close()
        })
    }

    #[test]
    fn closing_a_queue_keeps_its_budget_until_native_retirement() -> Result {
        with_output(|fixture| {
            let scope = fixture.grantor.capture().describe_delegated()?;
            let mut queue = queue(&fixture, 1)?;
            let mut others = KVec::new();
            for _ in 0..15 {
                others.push(
                    scope.create_queue(&fixture.renderer, &fixture.active, 1)?,
                    GFP_KERNEL,
                )?;
            }
            queue.queue_to(1, &fixture.destination, None)?;
            let mut native = ManualFence::new()?;
            queue
                .try_claim(&fixture.rendered)
                .ok_or(EINVAL)?
                .claim
                .release(Completion::Submitted(native.fence()));
            drop(queue);
            check(
                scope
                    .create_queue(&fixture.renderer, &fixture.active, 1)
                    .err()
                    == Some(EBUSY),
            )?;
            native.complete(Ok(()))?;
            let start = Instant::<Monotonic>::now();
            loop {
                match scope.create_queue(&fixture.renderer, &fixture.active, 1) {
                    Ok(queue) => {
                        drop(queue);
                        break;
                    }
                    Err(EBUSY) if start.elapsed() < Delta::from_secs(2) => {
                        fsleep(Delta::from_millis(1))
                    }
                    Err(error) => return Err(error),
                }
            }
            Ok(())
        })
    }

    #[test]
    fn acknowledged_maximum_id_requires_a_new_queue_incarnation() -> Result {
        with_output(|fixture| {
            let mut queue = queue(&fixture, 1)?;
            queue.queue_to(u64::MAX, &fixture.destination, None)?;
            queue.cancel(u64::MAX)?;
            check(queue.advance() == 1)?;
            queue.dequeue(|result| check(result.result == Err(ECANCELED)))?;
            check(queue.queue_to(1, &fixture.destination, None) == Err(EOVERFLOW))
        })
    }
}
