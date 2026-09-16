// SPDX-License-Identifier: GPL-2.0-only

use super::{
    delegated_authority::grant,
    private_images::{activate, buffer},
    *,
};
use crate::{
    capture::provider::{
        delegated_destination::Image as Destination,
        delegated_request::{Request, Status},
        Grantor,
    },
    renderer::{job::Completion, private_image::Image, render_job::Rendered},
    renderer_startup::Active,
};
use kernel::{
    dma_fence::testing::ManualFence,
    drm::{
        fourcc,
        gem::{BaseObject, ExportAccess},
    },
    time::{delay::fsleep, Delta, Instant, Monotonic},
};

struct Fixture {
    owner: Owner,
    renderer: Arc<Candidate>,
    active: Active,
    execution: crate::execution::Description,
    grantor: Grantor,
    private: Arc<Image>,
    rendered: Arc<Rendered>,
    destination: Arc<Destination>,
}

fn with_output(f: impl FnOnce(Fixture) -> Result) -> Result {
    with_display(|device, crtc, connector, _, file| {
        f(output_fixture(device, crtc, connector, &file)?)
    })
}

fn output_fixture(
    device: &Device<Driver, Registered>,
    crtc: &Crtc<display::Crtc>,
    connector: &Connector<display::Connector>,
    file: &RegisteredMasterFile<'_, Driver>,
) -> Result<Fixture> {
    let owner = owner(file, crtc, connector)?;
    let renderer = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
    let (active, execution) = activate(&renderer, device, crtc)?;
    let grantor = grant(file, crtc, connector)?;
    let private = renderer.register_private_image(
        &active,
        [640, 480],
        &[buffer(device, ExportAccess::ReadWrite)?],
    )?;
    let rendered = Arc::new(
        renderer
            .claim_render(&active, execution, None, private.prepare(1)?)?
            .release(Completion::Cpu)
            .ok_or(EINVAL)?,
        GFP_KERNEL,
    )?;
    let destination = grantor
        .capture()
        .describe_delegated()?
        .register_destination(
            &buffer(device, ExportAccess::ReadWrite)?,
            fourcc::XRGB8888,
            0,
            2560,
            0,
        )?;
    Ok(Fixture {
        owner,
        renderer,
        active,
        execution,
        grantor,
        private,
        rendered,
        destination,
    })
}

pub(super) fn wait(request: &Request, expected: Status) -> Result {
    let start = Instant::<Monotonic>::now();
    while request.status() == Status::Pending {
        if start.elapsed() > Delta::from_secs(2) {
            return Err(ETIMEDOUT);
        }
        fsleep(Delta::from_millis(1));
    }
    check(request.status() == expected)
}

fn private_available(image: &Arc<Image>, use_id: u64) -> Result {
    let start = Instant::<Monotonic>::now();
    loop {
        match image.prepare(use_id) {
            Ok(prepared) => {
                drop(prepared);
                return Ok(());
            }
            Err(EBUSY) if start.elapsed() < Delta::from_secs(2) => fsleep(Delta::from_millis(1)),
            Err(error) => return Err(error),
        }
    }
}

#[kunit_tests(rust_castkms_delegated_requests)]
mod cases {
    use super::*;

    #[test]
    fn queued_worker_loss_reconciles_without_claiming_private_content() -> Result {
        with_output(|fixture| {
            let observation = fixture.active.observation();
            let request = fixture.destination.request(1, None)?;
            drop(fixture.rendered);
            drop(fixture.active);
            check(
                request.status_for(&fixture.renderer, &observation) == Status::Complete(Err(EIO)),
            )?;
            check(request.native_completion().is_none())?;
            drop(fixture.destination.reserve(2, None)?);
            private_available(&fixture.private, 2)
        })
    }

    #[test]
    fn observed_claim_does_not_keep_the_worker_active() -> Result {
        with_output(|fixture| {
            let observation = fixture.active.observation();
            let request = fixture.destination.request(1, None)?;
            let claim = request
                .try_claim_observed(&fixture.renderer, &observation, &fixture.rendered)?
                .ok_or(EINVAL)?;
            drop(fixture.active);
            check(request.status_for(&fixture.renderer, &observation) == Status::Pending)?;
            claim.release(Completion::WithoutAccess);
            check(request.status_for(&fixture.renderer, &observation) == Status::Complete(Err(EIO)))
        })
    }

    #[test]
    fn rejected_request_admission_does_not_consume_a_destination_use() -> Result {
        with_output(|fixture| {
            check(fixture.destination.request(0, None).err() == Some(EINVAL))?;
            let first = fixture.destination.request(1, None)?;
            check(fixture.destination.request(2, None).err() == Some(EBUSY))?;
            first.cancel();
            let second = fixture.destination.request(2, None)?;
            second.cancel();
            check(second.status() == Status::Complete(Err(ECANCELED)))
        })
    }

    #[test]
    fn destination_becoming_a_source_is_rejected_at_output_claim() -> Result {
        with_display(|device, crtc, connector, scanout, file| {
            let fixture = output_fixture(device, crtc, connector, &file)?;
            let object = shmem::Object::<gem::Object>::new(
                device,
                640 * 480 * 4,
                Default::default(),
                Default::default(),
            )?;
            let destination = fixture
                .grantor
                .capture()
                .describe_delegated()?
                .register_destination(
                    &object.export_dma_buf(ExportAccess::ReadWrite)?,
                    fourcc::XRGB8888,
                    0,
                    2560,
                    0,
                )?;
            let request = destination.request(1, None)?;
            let framebuffer = Framebuffer::from_objects(
                device,
                &FramebufferLayout {
                    width: 640,
                    height: 480,
                    format: fourcc::XRGB8888,
                    modifier: None,
                    interlaced: false,
                    planes: &[FramebufferPlane {
                        object: &object,
                        pitch: 2560,
                        offset: 0,
                    }],
                },
            )?;
            device.atomic_update(|transaction| {
                transaction.set_crtc_config(
                    crtc,
                    Some(&CrtcScanout {
                        mode: scanout.mode,
                        framebuffer: &framebuffer,
                        connectors: scanout.connectors,
                        position: scanout.position,
                    }),
                )
            })?;
            check(
                request
                    .try_claim(&fixture.renderer, &fixture.active, &fixture.rendered)
                    .err()
                    == Some(EINVAL),
            )?;
            check(request.status() == Status::Complete(Err(EINVAL)))?;
            check(request.native_completion().is_none())?;
            // A rejected claim creates no output access and returns destination capacity.
            device.atomic_update(|transaction| transaction.set_crtc_config(crtc, Some(scanout)))?;
            drop(destination.reserve(2, None)?);
            Ok(())
        })
    }

    #[test]
    fn closing_master_preserves_native_cleanup_but_not_frame_authority() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let fixture = output_fixture(device, crtc, connector, &file)?;
            let request = fixture.destination.request(1, None)?;
            let mut native = ManualFence::new()?;
            request
                .try_claim(&fixture.renderer, &fixture.active, &fixture.rendered)?
                .ok_or(EINVAL)?
                .release(Completion::Submitted(native.fence()));
            drop(fixture.rendered);
            drop(file);
            check(request.status() == Status::Pending)?;
            check(fixture.private.prepare(2).err() == Some(EBUSY))?;
            native.complete(Ok(()))?;
            wait(&request, Status::Complete(Err(EACCES)))?;
            check(request.content_serial().is_none())?;
            private_available(&fixture.private, 2)
        })
    }

    #[test]
    fn device_shutdown_does_not_shortcut_native_output_retirement() -> Result {
        let display = CastKms::new(c"castkms-delegated-shutdown")?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let fixture = output_fixture(device, crtc, connector, &file)?;
            let request = fixture.destination.request(1, None)?;
            let mut native = ManualFence::new()?;
            request
                .try_claim(&fixture.renderer, &fixture.active, &fixture.rendered)?
                .ok_or(EINVAL)?
                .release(Completion::Submitted(native.fence()));
            drop(fixture.rendered);
            display.state.close();
            check(request.status() == Status::Pending)?;
            check(fixture.private.prepare(2).err() == Some(EBUSY))?;
            native.complete(Ok(()))?;
            wait(&request, Status::Complete(Err(ENODEV)))?;
            check(request.content_serial().is_none())?;
            check(fixture.destination.reserve(2, None).err() == Some(ENODEV))?;
            private_available(&fixture.private, 2)
        })
    }

    #[test]
    fn independent_recipients_retire_shared_private_storage_separately() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let renderer = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let (active, execution) = activate(&renderer, device, crtc)?;
            let first = grant(&file, crtc, connector)?;
            let second = grant(&file, crtc, connector)?;
            let private = renderer.register_private_image(
                &active,
                [640, 480],
                &[buffer(device, ExportAccess::ReadWrite)?],
            )?;
            let rendered = Arc::new(
                renderer
                    .claim_render(&active, execution, None, private.prepare(1)?)?
                    .release(Completion::Cpu)
                    .ok_or(EINVAL)?,
                GFP_KERNEL,
            )?;
            let serial = rendered.content().content_serial();
            let mut destinations = KVec::new();
            for grantor in [&first, &second] {
                destinations.push(
                    grantor
                        .capture()
                        .describe_delegated()?
                        .register_destination(
                            &buffer(device, ExportAccess::ReadWrite)?,
                            fourcc::XRGB8888,
                            0,
                            2560,
                            0,
                        )?,
                    GFP_KERNEL,
                )?;
            }
            let first_request = destinations[0].request(1, None)?;
            let second_request = destinations[1].request(1, None)?;
            let mut first_native = ManualFence::new()?;
            let mut second_native = ManualFence::new()?;
            first_request
                .try_claim(&renderer, &active, &rendered)?
                .ok_or(EINVAL)?
                .release(Completion::Submitted(first_native.fence()));
            second_request
                .try_claim(&renderer, &active, &rendered)?
                .ok_or(EINVAL)?
                .release(Completion::Submitted(second_native.fence()));
            drop(rendered);
            drop(first);
            first_native.complete(Ok(()))?;
            wait(&first_request, Status::Complete(Err(EKEYREVOKED)))?;
            check(first_request.content_serial().is_none())?;
            check(second_request.status() == Status::Pending)?;
            check(private.prepare(2).err() == Some(EBUSY))?;
            check(destinations[1].reserve(2, None).err() == Some(EBUSY))?;

            second_native.complete(Ok(()))?;
            wait(&second_request, Status::Complete(Ok(())))?;
            check(second_request.content_serial() == serial)?;
            private_available(&private, 2)?;
            let next = destinations[1].request(2, None)?;
            // Old terminal handles must not release the next use's exclusion.
            drop(first_request);
            drop(second_request);
            check(destinations[1].reserve(3, None).err() == Some(EBUSY))?;
            next.cancel();
            drop(destinations[1].reserve(3, None)?);
            Ok(())
        })
    }

    #[test]
    fn pending_destination_does_not_retain_private_storage() -> Result {
        with_output(|fixture| {
            let mut reuse = ManualFence::new()?;
            let request = fixture.destination.request(1, Some(reuse.fence()))?;
            check(
                request
                    .try_claim(&fixture.renderer, &fixture.active, &fixture.rendered)?
                    .is_none(),
            )?;
            drop(fixture.rendered);
            private_available(&fixture.private, 2)?;
            check(request.status() == Status::Pending)?;
            reuse.complete(Ok(()))?;
            request.cancel();
            check(request.status() == Status::Complete(Err(ECANCELED)))
        })
    }

    #[test]
    fn native_output_keeps_private_storage_until_access_ends() -> Result {
        for result in [Ok(()), Err(EIO), Err(EAGAIN)] {
            with_output(|fixture| {
                let request = fixture.destination.request(1, None)?;
                let serial = fixture.rendered.content().content_serial();
                let mut native = ManualFence::new()?;
                let claim = request
                    .try_claim(&fixture.renderer, &fixture.active, &fixture.rendered)?
                    .ok_or(EINVAL)?;
                let exact_resources = core::ptr::eq(claim.source(), &*fixture.rendered)
                    && core::ptr::eq(claim.destination(), &*fixture.destination);
                claim.release(Completion::Submitted(native.fence()));
                check(exact_resources)?;
                drop(fixture.rendered);
                check(request.status() == Status::Pending)?;
                check(fixture.private.prepare(2).err() == Some(EBUSY))?;
                check(fixture.destination.reserve(2, None).err() == Some(EBUSY))?;
                native.complete(result)?;
                wait(&request, Status::Complete(result))?;
                private_available(&fixture.private, 2)?;
                check(request.content_serial() == if result.is_ok() { serial } else { None })?;
                drop(fixture.destination.reserve(2, None)?);
                Ok(())
            })?;
        }
        Ok(())
    }

    #[test]
    fn cancellation_after_claim_waits_for_native_access() -> Result {
        with_output(|fixture| {
            let request = fixture.destination.request(1, None)?;
            let claim = request
                .try_claim(&fixture.renderer, &fixture.active, &fixture.rendered)?
                .ok_or(EINVAL)?;
            request.cancel();
            let mut native = ManualFence::new()?;
            claim.release(Completion::Submitted(native.fence()));
            drop(fixture.rendered);
            check(request.status() == Status::Pending)?;
            check(fixture.private.prepare(2).err() == Some(EBUSY))?;
            native.complete(Ok(()))?;
            wait(&request, Status::Complete(Err(ECANCELED)))?;
            private_available(&fixture.private, 2)
        })
    }

    #[test]
    fn revoked_claim_may_retire_but_cannot_publish_a_new_frame() -> Result {
        with_output(|fixture| {
            let request = fixture.destination.request(1, None)?;
            check(request.native_completion().is_none())?;
            let claim = request
                .try_claim(&fixture.renderer, &fixture.active, &fixture.rendered)?
                .ok_or(EINVAL)?;
            drop(fixture.grantor);
            let mut native = ManualFence::new()?;
            claim.release(Completion::Submitted(native.fence()));
            let completion = request.native_completion().ok_or(EINVAL)?;
            check(core::ptr::eq(&*completion, &*native.fence()))?;
            check(completion.status() == kernel::dma_fence::Status::Pending)?;
            drop(fixture.rendered);
            check(request.status() == Status::Pending)?;
            native.complete(Ok(()))?;
            wait(&request, Status::Complete(Err(EKEYREVOKED)))?;
            check(completion.status() == kernel::dma_fence::Status::Complete(Ok(())))?;
            check(request.content_serial().is_none())?;
            private_available(&fixture.private, 2)
        })
    }

    #[test]
    fn dropping_request_handle_does_not_release_native_ownership() -> Result {
        with_output(|fixture| {
            let request = fixture.destination.request(1, None)?;
            let claim = request
                .try_claim(&fixture.renderer, &fixture.active, &fixture.rendered)?
                .ok_or(EINVAL)?;
            let mut native = ManualFence::new()?;
            claim.release(Completion::Submitted(native.fence()));
            drop(request);
            drop(fixture.rendered);
            check(fixture.private.prepare(2).err() == Some(EBUSY))?;
            check(fixture.destination.reserve(2, None).err() == Some(EBUSY))?;
            native.complete(Ok(()))?;
            private_available(&fixture.private, 2)?;
            drop(fixture.destination.reserve(2, None)?);
            Ok(())
        })
    }

    #[test]
    fn losing_active_ownership_does_not_invalidate_native_cleanup() -> Result {
        with_output(|fixture| {
            let request = fixture.destination.request(1, None)?;
            let claim = request
                .try_claim(&fixture.renderer, &fixture.active, &fixture.rendered)?
                .ok_or(EINVAL)?;
            let mut native = ManualFence::new()?;
            claim.release(Completion::Submitted(native.fence()));
            drop(fixture.active);
            drop(fixture.rendered);
            check(request.status() == Status::Pending)?;
            native.complete(Ok(()))?;
            wait(&request, Status::Complete(Err(EIO)))?;
            private_available(&fixture.private, 2)
        })
    }

    #[test]
    fn revoked_renderer_is_rejected_even_while_destination_reuse_waits() -> Result {
        with_output(|fixture| {
            let reuse = ManualFence::new()?;
            let request = fixture.destination.request(1, Some(reuse.fence()))?;
            fixture.owner.revoke();
            check(
                request
                    .try_claim(&fixture.renderer, &fixture.active, &fixture.rendered)
                    .err()
                    == Some(EKEYREVOKED),
            )?;
            check(request.status() == Status::Complete(Err(EKEYREVOKED)))
        })
    }

    #[test]
    fn queued_revocation_reconciles_without_waiting_for_destination() -> Result {
        with_output(|fixture| {
            let reuse = ManualFence::new()?;
            let request = fixture.destination.request(1, Some(reuse.fence()))?;
            drop(fixture.grantor);
            check(request.status() == Status::Complete(Err(EKEYREVOKED)))
        })
    }

    #[test]
    fn reuse_error_is_terminal_instead_of_retried_as_pending() -> Result {
        with_output(|fixture| {
            let mut reuse = ManualFence::new()?;
            let request = fixture.destination.request(1, Some(reuse.fence()))?;
            reuse.complete(Err(EAGAIN))?;
            check(
                request
                    .try_claim(&fixture.renderer, &fixture.active, &fixture.rendered)
                    .err()
                    == Some(EAGAIN),
            )?;
            check(request.status() == Status::Complete(Err(EAGAIN)))?;
            drop(fixture.destination.reserve(2, None)?);
            Ok(())
        })
    }

    #[test]
    fn cpu_and_no_access_reports_release_both_stages_synchronously() -> Result {
        for access in [true, false] {
            with_output(|fixture| {
                let request = fixture.destination.request(1, None)?;
                let claim = request
                    .try_claim(&fixture.renderer, &fixture.active, &fixture.rendered)?
                    .ok_or(EINVAL)?;
                claim.release(if access {
                    Completion::Cpu
                } else {
                    Completion::WithoutAccess
                });
                drop(fixture.rendered);
                check(request.native_completion().is_none())?;
                check(
                    request.status()
                        == Status::Complete(if access { Ok(()) } else { Err(ECANCELED) }),
                )?;
                private_available(&fixture.private, 2)?;
                drop(fixture.destination.reserve(2, None)?);
                Ok(())
            })?;
        }
        Ok(())
    }

    #[test]
    fn request_has_only_one_claim_and_one_terminal_result() -> Result {
        with_output(|fixture| {
            let request = fixture.destination.request(1, None)?;
            let claim = request
                .try_claim(&fixture.renderer, &fixture.active, &fixture.rendered)?
                .ok_or(EINVAL)?;
            let duplicate = request
                .try_claim(&fixture.renderer, &fixture.active, &fixture.rendered)
                .err();
            // Resolve the real claim even if the duplicate check fails.
            claim.release(Completion::Cpu);
            check(duplicate == Some(EALREADY))?;
            request.cancel();
            check(request.status() == Status::Complete(Ok(())))?;
            check(
                request
                    .try_claim(&fixture.renderer, &fixture.active, &fixture.rendered)
                    .err()
                    == Some(EALREADY),
            )
        })
    }

    #[test]
    fn failed_private_production_never_becomes_an_output_claim() -> Result {
        with_output(|fixture| {
            drop(fixture.rendered);
            let mut native = ManualFence::new()?;
            let rendered = Arc::new(
                fixture
                    .renderer
                    .claim_render(
                        &fixture.active,
                        fixture.execution,
                        None,
                        fixture.private.prepare(2)?,
                    )?
                    .release(Completion::Submitted(native.fence()))
                    .ok_or(EINVAL)?,
                GFP_KERNEL,
            )?;
            let request = fixture.destination.request(1, None)?;
            check(
                request
                    .try_claim(&fixture.renderer, &fixture.active, &rendered)?
                    .is_none(),
            )?;
            native.complete(Err(EAGAIN))?;
            check(
                request
                    .try_claim(&fixture.renderer, &fixture.active, &rendered)
                    .err()
                    == Some(EAGAIN),
            )?;
            check(request.status() == Status::Complete(Err(EAGAIN)))?;
            drop(rendered);
            private_available(&fixture.private, 3)
        })
    }

    #[test]
    fn animation_continues_while_a_separate_output_write_is_pending() -> Result {
        with_display(|device, crtc, connector, scanout, file| {
            let owner = owner(&file, crtc, connector)?;
            let renderer = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let (active, execution) = activate(&renderer, device, crtc)?;
            let grantor = grant(&file, crtc, connector)?;
            let private = renderer.register_private_image(
                &active,
                [640, 480],
                &[buffer(device, ExportAccess::ReadWrite)?],
            )?;
            let independent = renderer.register_private_image(
                &active,
                [640, 480],
                &[buffer(device, ExportAccess::ReadWrite)?],
            )?;
            let rendered = Arc::new(
                renderer
                    .claim_render(&active, execution, None, private.prepare(1)?)?
                    .release(Completion::Cpu)
                    .ok_or(EINVAL)?,
                GFP_KERNEL,
            )?;
            let destination = grantor
                .capture()
                .describe_delegated()?
                .register_destination(
                    &buffer(device, ExportAccess::ReadWrite)?,
                    fourcc::XRGB8888,
                    0,
                    2560,
                    0,
                )?;
            let request = destination.request(1, None)?;
            let claim = request
                .try_claim(&renderer, &active, &rendered)?
                .ok_or(EINVAL)?;
            let mut native = ManualFence::new()?;
            claim.release(Completion::Submitted(native.fence()));
            let original = rendered.content().content_serial();
            drop(rendered);
            let mut previous = original;
            for use_id in 1..=8 {
                device.atomic_update(|transaction| {
                    transaction.set_crtc_config(crtc, Some(scanout))
                })?;
                let job = renderer.claim_render(
                    &active,
                    execution,
                    None,
                    independent.prepare(use_id)?,
                )?;
                let frame = job.release(Completion::Cpu).ok_or(EINVAL)?;
                check(frame.content().content_serial() != previous)?;
                previous = frame.content().content_serial();
                drop(frame);
                check(request.status() == Status::Pending)?;
                check(private.prepare(2).err() == Some(EBUSY))?;
            }
            native.complete(Ok(()))?;
            wait(&request, Status::Complete(Ok(())))?;
            check(request.content_serial() == original)?;
            private_available(&private, 2)
        })
    }
}
