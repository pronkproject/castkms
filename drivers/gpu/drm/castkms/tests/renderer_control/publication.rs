// SPDX-License-Identifier: GPL-2.0-only

//! Prepared metadata is consumed only by its own current publication under renderer control.

use super::*;
use crate::{
    execution::Profile,
    renderer::{
        job::Completion,
        session::Session, //
    },
};
use kernel::{
    dma_fence::testing::ManualFence,
    drm::gem::BaseObject,
    sync::aref::ARef, //
};

#[kunit_tests(rust_castkms_renderer_publication)]
mod cases {
    use super::*;

    fn linear_profile() -> Result<crate::execution::capabilities::Profile> {
        let reference = crate::tests::renderer_proposals::profile()?;
        let mut formats = KVec::new();
        formats.push(
            crate::execution::capabilities::Format {
                fourcc: drm::fourcc::XRGB8888,
                modifier: None,
                planes: 1,
                native: true,
                imported: true,
                pitch_alignment: 1,
                offset_alignment: 1,
                max_pitch: u32::MAX,
            },
            GFP_KERNEL,
        )?;
        crate::execution::capabilities::Profile::new(*reference.limits(), formats)
    }

    fn gate_profile(
        candidate: &Arc<Candidate>,
        device: &Device<Driver, Registered>,
        crtc: &Crtc<display::Crtc>,
        host: bool,
    ) -> Result<crate::renderer::proposal::Proposal> {
        let proposal = if host {
            candidate.propose_host()?
        } else {
            candidate.propose_profile(linear_profile()?)?
        };
        device.atomic_update(|transaction| {
            transaction
                .add_crtc_state(crtc)?
                .tag_transition(proposal.describe().transition);
            Ok(())
        })?;
        Ok(proposal)
    }

    #[test]
    fn released_content_does_not_retain_source_reads_during_animation() -> Result {
        with_display(|device, crtc, connector, scanout, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            candidate.submit_private_probe(None)?;
            let proposal = gate_profile(&candidate, device, crtc, false)?;
            let (active, _, execution) = proposal.activate(device)?;
            for _ in 0..16 {
                let job = candidate.claim_source(&active, execution, None)?;
                let serial = job.scene().content_serial();
                let source = device
                    .output
                    .with_accepted(|accepted| accepted.map(|item| ARef::from(item.source)))
                    .ok_or(EINVAL)?;
                let content = job.release(Completion::Cpu).ok_or(EINVAL)?;
                source.seal();
                check(source.prepared()?.is_some())?;
                device.atomic_update(|transaction| {
                    transaction.set_crtc_config(crtc, Some(scanout))
                })?;
                check(content.content_serial() == serial)?;
                candidate.with_content(&active, &content, || Ok(()))?;
            }
            Ok(())
        })
    }

    #[test]
    fn released_content_requires_successful_native_completion() -> Result {
        for result in [Ok(()), Err(EIO)] {
            with_display(|device, crtc, connector, _, file| {
                let owner = owner(&file, crtc, connector)?;
                let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
                candidate.submit_private_probe(None)?;
                let proposal = gate_profile(&candidate, device, crtc, false)?;
                let (active, _, execution) = proposal.activate(device)?;
                let job = candidate.claim_source(&active, execution, None)?;
                let mut native = ManualFence::new()?;
                let content = job
                    .release(Completion::Submitted(native.fence()))
                    .ok_or(EINVAL)?;
                check(matches!(
                    content.status(),
                    kernel::dma_fence::Status::Pending
                ))?;
                let mut calls = 0;
                check(
                    candidate.with_content(&active, &content, || {
                        calls += 1;
                        Ok(())
                    }) == Err(EAGAIN),
                )?;
                check(calls == 0)?;
                native.complete(result)?;
                check(candidate.with_content(&active, &content, || Ok(())) == result)
            })?;
        }
        Ok(())
    }

    #[test]
    fn producer_failure_is_not_erased_by_successful_rendering() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            candidate.submit_private_probe(None)?;
            let proposal = gate_profile(&candidate, device, crtc, false)?;
            let (active, _, execution) = proposal.activate(device)?;
            let mut producer = ManualFence::new()?;
            producer.complete(Err(EIO))?;
            device.atomic_update(|transaction| {
                transaction
                    .add_plane_state(crtc.primary_plane())?
                    .set_producer_fence(Some(producer.fence()));
                Ok(())
            })?;
            let job = candidate.claim_source(&active, execution, None)?;
            let source = device
                .output
                .with_accepted(|accepted| accepted.map(|item| ARef::from(item.source)))
                .ok_or(EINVAL)?;
            let mut native = ManualFence::new()?;
            let content = job
                .release(Completion::Submitted(native.fence()))
                .ok_or(EINVAL)?;
            source.seal();
            let prepared = source.prepared()?.ok_or(EAGAIN)?;
            let retirement = prepared.completion()?.ok_or(EINVAL)?;
            check(matches!(
                retirement.status(),
                kernel::dma_fence::Status::Pending
            ))?;
            check(candidate.with_content(&active, &content, || Ok(())) == Err(EIO))?;
            native.complete(Ok(()))?;
            check(matches!(
                retirement.status(),
                kernel::dma_fence::Status::Complete(Ok(()))
            ))?;
            check(candidate.with_content(&active, &content, || Ok(())) == Err(EIO))?;
            // A fresh source use has its own producer records, not a persistent image error.
            device.atomic_update(|transaction| {
                transaction.add_plane_state(crtc.primary_plane())?;
                Ok(())
            })?;
            let next = candidate
                .claim_source(&active, execution, None)?
                .release_cpu();
            candidate.with_content(&active, &next, || Ok(()))
        })
    }

    #[test]
    fn no_access_release_produces_no_content_evidence() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            candidate.submit_private_probe(None)?;
            let proposal = gate_profile(&candidate, device, crtc, false)?;
            let (active, _, execution) = proposal.activate(device)?;
            let job = candidate.claim_source(&active, execution, None)?;
            check(job.release(Completion::WithoutAccess).is_none())
        })
    }

    #[test]
    fn released_content_requires_live_authority_and_the_same_configuration() -> Result {
        for revoke in [false, true] {
            with_display(|device, crtc, connector, scanout, file| {
                let owner = owner(&file, crtc, connector)?;
                let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
                candidate.submit_private_probe(None)?;
                let proposal = gate_profile(&candidate, device, crtc, false)?;
                let (active, _, execution) = proposal.activate(device)?;
                let content = candidate
                    .claim_source(&active, execution, None)?
                    .release_cpu();
                if revoke {
                    owner.revoke();
                } else {
                    device.atomic_update(|transaction| transaction.set_crtc_config(crtc, None))?;
                    device.atomic_update(|transaction| {
                        transaction.set_crtc_config(crtc, Some(scanout))
                    })?;
                }
                let mut called = false;
                check(
                    candidate.with_content(&active, &content, || {
                        called = true;
                        Ok(())
                    }) == Err(if revoke { EKEYREVOKED } else { ESTALE }),
                )?;
                check(!called)
            })?;
        }
        Ok(())
    }

    #[test]
    fn replacement_renderer_cannot_adopt_content_evidence() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            candidate.submit_private_probe(None)?;
            let proposal = gate_profile(&candidate, device, crtc, false)?;
            let (active, _, execution) = proposal.activate(device)?;
            let content = candidate
                .claim_source(&active, execution, None)?
                .release_cpu();
            let replacement = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            replacement.submit_private_probe(None)?;
            let proposal = gate_profile(&replacement, device, crtc, false)?;
            let (next, _, _) = proposal.activate(device)?;
            check(candidate.with_content(&active, &content, || Ok(())) == Err(EIO))?;
            check(replacement.with_content(&next, &content, || Ok(())) == Err(ESTALE))
        })
    }

    #[test]
    fn another_output_cannot_adopt_content_evidence() -> Result {
        let other = CastKms::new(c"castkms-content-other-output")?;
        with_display(|device, crtc, connector, _, file| {
            let authority = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(authority.access())?, GFP_KERNEL)?;
            candidate.submit_private_probe(None)?;
            let proposal = gate_profile(&candidate, device, crtc, false)?;
            let (active, _, execution) = proposal.activate(device)?;
            let content = candidate
                .claim_source(&active, execution, None)?
                .release_cpu();
            with_registered_display(&other, |other, crtc, connector, _, file| {
                let authority = owner(&file, crtc, connector)?;
                let candidate = Arc::new(Candidate::begin(authority.access())?, GFP_KERNEL)?;
                candidate.submit_private_probe(None)?;
                let proposal = gate_profile(&candidate, other, crtc, false)?;
                let (active, _, _) = proposal.activate(other)?;
                check(candidate.with_content(&active, &content, || Ok(())) == Err(EACCES))
            })
        })
    }

    #[test]
    fn negotiated_gpu_contract_accepts_tiling_float_and_larger_modes() -> Result {
        with_display(|device, crtc, connector, scanout, file| {
            let owner = owner(&file, crtc, connector)?;
            let reference = linear_profile()?;
            let mut formats = KVec::new();
            formats.push(reference.formats()[0], GFP_KERNEL)?;
            let tiled_modifier = 0x0100_0000_0000_0001;
            for (format, modifier, pitch) in [
                (drm::fourcc::XRGB8888, Some(tiled_modifier), 2560),
                (drm::fourcc::XRGB16161616F, None, 5120),
            ] {
                let mut tuple = reference.formats()[0];
                tuple.fourcc = format;
                tuple.modifier = modifier;
                tuple.max_pitch = pitch;
                formats.push(tuple, GFP_KERNEL)?;
            }
            let profile =
                crate::execution::capabilities::Profile::new(*reference.limits(), formats)?;
            let tiled_object = shmem::Object::<gem::Object>::new(
                device,
                2560 * 480,
                Default::default(),
                Default::default(),
            )?;
            let tiled_framebuffer = Framebuffer::from_objects(
                device,
                &FramebufferLayout {
                    width: 640,
                    height: 480,
                    format: drm::fourcc::XRGB8888,
                    modifier: Some(tiled_modifier),
                    interlaced: false,
                    planes: &[FramebufferPlane {
                        object: &tiled_object,
                        pitch: 2560,
                        offset: 0,
                    }],
                },
            )?;
            let tiled_scanout = CrtcScanout {
                mode: scanout.mode,
                framebuffer: &tiled_framebuffer,
                connectors: scanout.connectors,
                position: (0, 0),
            };
            // The static GPU envelope must not broaden HOST acceptance.
            check(
                device
                    .atomic_update(|transaction| {
                        transaction.set_crtc_config(crtc, Some(&tiled_scanout))
                    })
                    .is_err(),
            )?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            candidate.submit_private_probe(None)?;
            let proposal = candidate.propose_profile(profile)?;
            device.atomic_update(|transaction| {
                transaction
                    .add_crtc_state(crtc)?
                    .tag_transition(proposal.describe().transition);
                Ok(())
            })?;
            let (active, _, execution) = proposal.activate(device)?;
            for (format, modifier, pitch) in [
                (drm::fourcc::XRGB8888, Some(tiled_modifier), 2560),
                (drm::fourcc::XRGB16161616F, None, 5120),
            ] {
                let object = shmem::Object::<gem::Object>::new(
                    device,
                    pitch as usize * 480,
                    Default::default(),
                    Default::default(),
                )?;
                let framebuffer = Framebuffer::from_objects(
                    device,
                    &FramebufferLayout {
                        width: 640,
                        height: 480,
                        format,
                        modifier,
                        interlaced: false,
                        planes: &[FramebufferPlane {
                            object: &object,
                            pitch,
                            offset: 0,
                        }],
                    },
                )?;
                let target = CrtcScanout {
                    mode: scanout.mode,
                    framebuffer: &framebuffer,
                    connectors: scanout.connectors,
                    position: (0, 0),
                };
                device.atomic_update(|transaction| {
                    transaction.set_crtc_config(crtc, Some(&target))
                })?;
                let job = candidate.claim_source(&active, execution, None)?;
                check(
                    job.scene()
                        .primary()
                        .ok_or(EINVAL)?
                        .framebuffer()
                        .modifier()
                        == modifier,
                )?;
                job.release_without_access();
            }
            let mode = DisplayMode::from_timings(ModeTimings {
                clock_khz: 300000,
                hdisplay: 9000,
                hsync_start: 9016,
                hsync_end: 9112,
                htotal: 9200,
                vdisplay: 480,
                vsync_start: 490,
                vsync_end: 492,
                vtotal: 525,
                flags: ModeFlags::NHSYNC | ModeFlags::NVSYNC,
            })?;
            let target = CrtcScanout {
                mode: &mode,
                framebuffer: scanout.framebuffer,
                connectors: scanout.connectors,
                position: (0, 0),
            };
            device.atomic_update(|mut transaction| {
                transaction.as_mut().set_crtc_config(crtc, Some(&target))?;
                transaction.as_mut().disable_plane(crtc.primary_plane())
            })?;
            active.check()?;
            let incoming = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let host = incoming.propose_host()?;
            check(
                device
                    .atomic_update(|transaction| {
                        transaction
                            .add_crtc_state(crtc)?
                            .tag_transition(host.describe().transition);
                        Ok(())
                    })
                    .is_err(),
            )?;
            device.atomic_update(|mut transaction| {
                transaction.as_mut().set_crtc_config(crtc, None)?;
                transaction
                    .add_crtc_state(crtc)?
                    .tag_transition(host.describe().transition);
                Ok(())
            })?;
            host.handback(device)?;
            device.execution.check_host()
        })
    }

    #[test]
    fn host_handback_uses_a_gate_without_releasing_old_source_reads() -> Result {
        for disable in [false, true] {
            with_display(|device, crtc, connector, _, file| {
                let owner = owner(&file, crtc, connector)?;
                let first = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
                first.submit_private_probe(None)?;
                let proposal = first.propose_profile(linear_profile()?)?;
                device.atomic_update(|transaction| {
                    transaction
                        .add_crtc_state(crtc)?
                        .tag_transition(proposal.describe().transition);
                    Ok(())
                })?;
                let (old, _, description) = proposal.activate(device)?;
                let incoming = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
                let handback = incoming.propose_host()?;
                check(handback.handback(device) == Err(EAGAIN))?;
                old.check()?;
                device.atomic_update(|mut transaction| {
                    if disable {
                        transaction.as_mut().set_crtc_config(crtc, None)?;
                    }
                    transaction
                        .add_crtc_state(crtc)?
                        .tag_transition(handback.describe().transition);
                    Ok(())
                })?;
                let job = if disable {
                    None
                } else {
                    Some(first.claim_source(&old, description, None)?)
                };
                let source = device
                    .output
                    .with_accepted(|accepted| accepted.map(|item| ARef::from(item.source)))
                    .ok_or(EINVAL)?;
                let host = handback.handback(device)?;
                check(
                    host.profile == Profile::HostV1
                        && host.generation == description.generation + 1,
                )?;
                check(old.check() == Err(EIO))?;
                device.execution.check_host()?;
                let next = device.startup.begin()?;
                drop(old);
                next.check()?;
                source.seal();
                if let Some(job) = job {
                    check(source.prepared()?.is_none())?;
                    job.release_without_access();
                }
                check(source.prepared()?.is_some())
            })?;
        }
        Ok(())
    }

    #[test]
    fn sessions_reconcile_host_handback_while_the_old_endpoint_drains() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let old = Session::new(owner.access(), device.to_registered_ref())?;
            let begin = old.begin(device.execution.describe().generation)?;
            let old_id = begin.id();
            begin.publish()?;
            old.candidate(old_id)?.submit_private_probe(None)?;
            let profile = old.propose_profile(old_id, linear_profile()?)?;
            device.atomic_update(|transaction| {
                transaction
                    .add_crtc_state(crtc)?
                    .tag_transition(profile.transition);
                Ok(())
            })?;
            let gpu = old.activate(old_id)?;
            let host = Session::new(owner.access(), device.to_registered_ref())?;
            let begin = host.begin(gpu.generation)?;
            let host_id = begin.id();
            begin.publish()?;
            let pending = host.propose_host(host_id)?;
            device.atomic_update(|transaction| {
                transaction
                    .add_crtc_state(crtc)?
                    .tag_transition(pending.transition);
                Ok(())
            })?;
            let read = old.begin_source()?;
            let read_id = read.id();
            read.publish(|| ())?;
            let result = host.activate(host_id)?;
            check(result.profile == Profile::HostV1)?;
            check(host.activate(host_id)? == result)?;
            check(host.abort(host_id) == Err(EALREADY))?;
            old.release_source(read_id, Completion::WithoutAccess)?;
            check(old.begin_source().err() == Some(EIO))?;
            drop(host.begin(result.generation)?);
            old.close_for_test();
            host.close_for_test();
            device.execution.check_host()
        })
    }

    #[test]
    fn replacing_a_negotiated_worker_retains_its_outstanding_source_read() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let first = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            first.submit_private_probe(None)?;
            let proposal = first.propose_profile(linear_profile()?)?;
            device.atomic_update(|transaction| {
                transaction
                    .add_crtc_state(crtc)?
                    .tag_transition(proposal.describe().transition);
                Ok(())
            })?;
            let (old, _, description) = proposal.activate(device)?;
            let replacement = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            replacement.submit_private_probe(None)?;
            let next = replacement.propose_profile(linear_profile()?)?;
            old.check()?;
            device.atomic_update(|transaction| {
                transaction
                    .add_crtc_state(crtc)?
                    .tag_transition(next.describe().transition);
                Ok(())
            })?;
            let job = first.claim_source(&old, description, None)?;
            let source = device
                .output
                .with_accepted(|accepted| accepted.map(|item| ARef::from(item.source)))
                .ok_or(EINVAL)?;
            let (active, _, next_description) = next.activate(device)?;
            check(next_description.generation == description.generation + 1)?;
            check(old.check() == Err(EIO))?;
            drop(old);
            active.check()?;
            source.seal();
            check(source.prepared()?.is_none())?;
            job.release_without_access();
            check(source.prepared()?.is_some())
        })
    }

    #[test]
    fn tagged_configuration_changes_keep_the_incoming_candidate() -> Result {
        for disable in [false, true] {
            with_display(|device, crtc, connector, scanout, file| {
                let owner = owner(&file, crtc, connector)?;
                let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
                candidate.submit_private_probe(None)?;
                let proposal =
                    candidate.propose_profile(crate::tests::renderer_proposals::profile()?)?;
                let mode = DisplayMode::from_timings(ModeTimings {
                    clock_khz: 24000,
                    hdisplay: 640,
                    hsync_start: 656,
                    hsync_end: 752,
                    htotal: 800,
                    vdisplay: 480,
                    vsync_start: 490,
                    vsync_end: 492,
                    vtotal: 525,
                    flags: ModeFlags::NHSYNC | ModeFlags::NVSYNC,
                })?;
                let target = CrtcScanout {
                    mode: &mode,
                    framebuffer: scanout.framebuffer,
                    connectors: scanout.connectors,
                    position: (0, 0),
                };
                device.atomic_update(|mut transaction| {
                    transaction
                        .as_mut()
                        .set_crtc_config(crtc, if disable { None } else { Some(&target) })?;
                    transaction.as_mut().disable_plane(crtc.primary_plane())?;
                    transaction
                        .add_crtc_state(crtc)?
                        .tag_transition(proposal.describe().transition);
                    Ok(())
                })?;
                let (active, _, description) = proposal.activate(device)?;
                active.check()?;
                check(description.profile == Profile::GpuV1)?;
                // Re-enable or change mode under the new contract without losing the worker.
                device.atomic_update(|mut transaction| {
                    transaction.as_mut().set_crtc_config(crtc, Some(scanout))?;
                    transaction.as_mut().disable_plane(crtc.primary_plane())
                })?;
                active.check()?;
                check(device.execution.pending_profile().is_none())
            })?;
        }
        Ok(())
    }

    #[test]
    fn session_retains_negotiation_across_activation_reply_retries() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let session = Session::new(owner.access(), device.to_registered_ref())?;
            let initial = session.capabilities()?;
            check(initial.validation.generation == 1 && initial.validation.epoch == 1)?;
            check(matches!(
                initial.validation.active,
                crate::execution::validation::Contract::Host
            ))?;
            let pending = session.begin(device.execution.describe().generation)?;
            let id = pending.id();
            pending.publish()?;
            session.candidate(id)?.submit_private_probe(None)?;
            let proposal =
                session.propose_profile(id, crate::tests::renderer_proposals::profile()?)?;
            let pending = session.capabilities()?;
            check(pending.validation.generation == initial.validation.generation)?;
            check(pending.validation.epoch == initial.validation.epoch)?;
            check(pending.validation.pending == Some((proposal.transition, false)))?;
            check(pending.pending.ok_or(EINVAL)?.generation == proposal.generation)?;
            check(session.pending_profile()?.ok_or(EINVAL)?.transition == proposal.transition)?;
            check(session.activate(id) == Err(EAGAIN))?;
            device.atomic_update(|mut transaction| {
                transaction.as_mut().disable_plane(crtc.primary_plane())?;
                transaction
                    .add_crtc_state(crtc)?
                    .tag_transition(proposal.transition);
                Ok(())
            })?;
            let gated = session.capabilities()?;
            check(gated.validation.pending == Some((proposal.transition, true)))?;
            check(gated.validation.epoch == initial.validation.epoch + 1)?;
            let description = session.activate(id)?;
            let activated = session.capabilities()?;
            check(activated.execution == description)?;
            check(activated.validation.generation == proposal.generation)?;
            check(activated.validation.epoch == initial.validation.epoch + 2)?;
            check(activated.validation.pending.is_none() && activated.pending.is_none())?;
            check(matches!(
                activated.validation.active,
                crate::execution::validation::Contract::Renderer(_)
            ))?;
            check(session.activate(id)? == description)?;
            check(session.pending_profile()?.is_none())?;
            check(session.abort(id) == Err(EALREADY))?;
            session.close_for_test();
            Ok(())
        })
    }

    #[test]
    fn session_abort_and_close_release_pending_profile_ownership() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let session = Session::new(owner.access(), device.to_registered_ref())?;
            for close in [false, true] {
                let pending = session.begin(device.execution.describe().generation)?;
                let id = pending.id();
                pending.publish()?;
                session.propose_profile(id, crate::tests::renderer_proposals::profile()?)?;
                check(session.pending_profile()?.is_some())?;
                if close {
                    session.close_for_test();
                } else {
                    session.abort(id)?;
                }
                check(device.execution.pending_profile().is_none())?;
            }
            Ok(())
        })
    }

    #[test]
    fn negotiated_activation_requires_a_published_gate() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            candidate.submit_private_probe(None)?;
            let proposal =
                candidate.propose_profile(crate::tests::renderer_proposals::profile()?)?;
            check(proposal.activate(device).err() == Some(EAGAIN))?;
            let before = device.execution.describe();
            device.atomic_update(|mut transaction| {
                transaction.as_mut().disable_plane(crtc.primary_plane())?;
                transaction
                    .add_crtc_state(crtc)?
                    .tag_transition(proposal.describe().transition);
                Ok(())
            })?;
            let (_active, _, description) = proposal.activate(device)?;
            check(description.generation == before.generation + 1)?;
            check(description.profile == Profile::GpuV1)?;
            check(device.execution.pending_profile().is_none())?;
            proposal.cancel();
            let scene = crate::scene::Scene::blank(None);
            device.validation.lock().check(
                0,
                crate::execution::validation::SceneView::Enabled {
                    scene: &scene,
                    output: [16384; 2],
                },
            )?;
            Ok(())
        })
    }

    #[test]
    fn activation_requires_a_registered_profile() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let session = Session::new(owner.access(), device.to_registered_ref())?;
            let pending = session.begin(device.execution.describe().generation)?;
            let id = pending.id();
            pending.publish()?;
            session.candidate(id)?.submit_private_probe(None)?;
            let before = device.execution.describe();
            check(session.activate(id) == Err(EINVAL))?;
            check(device.execution.describe() == before)?;
            let proposal = session.propose_profile(id, linear_profile()?)?;
            check(session.activate(id) == Err(EAGAIN))?;
            device.atomic_update(|transaction| {
                transaction
                    .add_crtc_state(crtc)?
                    .tag_transition(proposal.transition);
                Ok(())
            })?;
            let next = session.activate(id)?;
            check(next.generation == before.generation + 1)
        })
    }

    #[test]
    fn publishing_metadata_invalidates_the_old_candidate() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let before = device.execution.describe();
            let proposal = gate_profile(&candidate, device, crtc, true)?;
            let mut prepared = device.execution.prepare(device, Profile::HostV1)?;
            let next = prepared.description()?;
            check(next.generation == before.generation + 1)?;
            check(device.execution.describe() == before)?;
            owner
                .access()
                .with_installed_transition(device, |current, locked| {
                    device.execution.publish_proposal(
                        locked,
                        &mut prepared,
                        proposal.describe().generation,
                        current.configuration(),
                        |contract| current.check_contract(contract),
                    )
                })?;
            check(device.execution.describe() == next)?;
            check(prepared.description() == Err(EALREADY))?;
            check(candidate.validate() == Err(ESTALE))?;
            candidate.cancel();
            let replacement = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            replacement.validate()?;
            owner
                .access()
                .with_installed_transition(device, |current, locked| {
                    check(
                        device.execution.publish_proposal(
                            locked,
                            &mut prepared,
                            proposal.describe().generation,
                            current.configuration(),
                            |contract| current.check_contract(contract),
                        ) == Err(EALREADY),
                    )
                })
        })
    }

    #[test]
    fn gpu_publication_closes_new_host_source_admission() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let host = device.host.configure(
                device,
                crate::host_compositor::layout::Layout::new(640, 480)?,
            )?;
            candidate.submit_private_probe(None)?;
            let proposal = gate_profile(&candidate, device, crtc, false)?;
            let (_active, _, description) = proposal.activate(device)?;
            check(description == device.execution.describe())?;
            check(device.execution.admit_host().err() == Some(EOPNOTSUPP))?;
            let request = host.request_outcome()?;
            check(matches!(
                request.wait()?,
                crate::host_compositor::worker::Outcome::Failed(error)
                    if error == EOPNOTSUPP
            ))
        })
    }

    #[test]
    fn completed_probe_activation_transfers_device_wide_ownership() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let before = device.execution.describe();
            candidate.submit_private_probe(None)?;
            let proposal = gate_profile(&candidate, device, crtc, false)?;
            let (active, _source, description) = proposal.activate(device)?;
            check(description.generation == before.generation + 1)?;
            check(description.profile == Profile::GpuV1)?;
            check(description == device.execution.describe())?;
            active.check()?;
            let replacement = device.startup.begin()?;
            replacement.check()?;
            drop(replacement);
            drop(active);
            check(matches!(device.startup.begin(), Err(EBUSY)))
        })
    }

    #[test]
    fn active_renderer_claim_retires_only_after_explicit_release() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            candidate.submit_private_probe(None)?;
            let proposal = gate_profile(&candidate, device, crtc, false)?;
            let (active, _, description) = proposal.activate(device)?;
            let source = device
                .output
                .with_accepted(|accepted| accepted.map(|item| ARef::from(item.source)))
                .ok_or(EINVAL)?;
            check(
                candidate
                    .claim_source(&active, candidate.execution(), None)
                    .err()
                    == Some(ESTALE),
            )?;
            let job = candidate.claim_source(&active, description, None)?;
            check(job.scene().primary().is_some())?;
            source.seal();
            check(source.prepared()?.is_none())?;
            job.release_without_access();
            check(source.prepared()?.is_some())
        })
    }

    #[test]
    fn session_publishes_one_changed_source_until_release() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let session = Session::new(owner.access(), device.to_registered_ref())?;
            let pending = session.begin(device.execution.describe().generation)?;
            let candidate_id = pending.id();
            pending.publish()?;
            session
                .candidate(candidate_id)?
                .submit_private_probe(None)?;
            let proposal = session.propose_profile(candidate_id, linear_profile()?)?;
            device.atomic_update(|transaction| {
                transaction
                    .add_crtc_state(crtc)?
                    .tag_transition(proposal.transition);
                Ok(())
            })?;
            let execution = session.activate(candidate_id)?;
            check(execution.profile == Profile::GpuV1)?;

            let source = device
                .output
                .with_accepted(|accepted| accepted.map(|item| ARef::from(item.source)))
                .ok_or(EINVAL)?;
            let unpublished = session.begin_source()?;
            let unpublished_id = unpublished.id();
            drop(unpublished);
            let pending = session.begin_source()?;
            let job_id = pending.id();
            check(job_id == unpublished_id + 1)?;
            let description = pending.scene_description()?;
            check(description.layers.len() == 1)?;
            let layer = description.layers[0];
            check(layer.framebuffer().format() == drm::fourcc::XRGB8888)?;
            check(layer.framebuffer().modifier().is_none())?;
            check(layer.geometry().source == [0, 0, 640 << 16, 480 << 16])?;
            check(layer.geometry().destination == [640, 480])?;
            check(description.output == [640, 480])?;
            check(layer.framebuffer().plane_count() == 1)?;
            check(description.content_serial != 0)?;
            check(layer.framebuffer().pitch(0)? == 2560)?;
            check(layer.framebuffer().offset(0)? == 0)?;
            let buffer = layer.framebuffer().object_at(0)?.export_dma_buf(
                kernel::drm::gem::ExportAccess::ReadOnly,
            )?;
            check(!buffer.is_writable())?;
            check(session.begin_source().err() == Some(EBUSY))?;
            drop(description);
            let mut published = false;
            pending.publish(|| published = true)?;
            check(published)?;
            check(session.begin_source().err() == Some(EBUSY))?;
            check(session.release_source(job_id + 1, Completion::WithoutAccess) == Err(ENOENT))?;
            check(session.begin_source().err() == Some(EBUSY))?;

            source.seal();
            session.release_source(job_id, Completion::WithoutAccess)?;
            check(source.prepared()?.is_some())?;
            session.release_source(job_id, Completion::WithoutAccess)?;
            check(session.begin_source().err() == Some(ENODATA))
        })
    }

    #[test]
    fn dropped_renderer_job_fails_source_preparation() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            candidate.submit_private_probe(None)?;
            let proposal = gate_profile(&candidate, device, crtc, false)?;
            let (active, _, description) = proposal.activate(device)?;
            let source = device
                .output
                .with_accepted(|accepted| accepted.map(|item| ARef::from(item.source)))
                .ok_or(EINVAL)?;
            let job = candidate.claim_source(&active, description, None)?;
            source.seal();
            drop(job);
            check(matches!(source.prepared(), Err(EIO)))
        })
    }

    #[test]
    fn cpu_renderer_release_completes_source_preparation() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            candidate.submit_private_probe(None)?;
            let proposal = gate_profile(&candidate, device, crtc, false)?;
            let (active, _, description) = proposal.activate(device)?;
            let source = device
                .output
                .with_accepted(|accepted| accepted.map(|item| ARef::from(item.source)))
                .ok_or(EINVAL)?;
            let job = candidate.claim_source(&active, description, None)?;
            source.seal();
            drop(job.release(Completion::Cpu));
            let prepared = source.prepared()?.ok_or(EAGAIN)?;
            check(prepared.completion()?.is_none())
        })
    }

    #[test]
    fn submitted_renderer_release_transfers_native_completion() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            candidate.submit_private_probe(None)?;
            let proposal = gate_profile(&candidate, device, crtc, false)?;
            let (active, _, description) = proposal.activate(device)?;
            let source = device
                .output
                .with_accepted(|accepted| accepted.map(|item| ARef::from(item.source)))
                .ok_or(EINVAL)?;
            let job = candidate.claim_source(&active, description, None)?;
            let mut completion = ManualFence::new()?;
            let fence = completion.fence();
            source.seal();
            drop(job.release(Completion::Submitted(fence)));
            let prepared = source.prepared()?.ok_or(EAGAIN)?;
            let retained = prepared.completion()?.ok_or(EINVAL)?;
            check(matches!(
                retained.status(),
                kernel::dma_fence::Status::Pending
            ))?;
            completion.complete(Ok(()))?;
            check(matches!(
                retained.status(),
                kernel::dma_fence::Status::Complete(Ok(()))
            ))
        })
    }

    #[test]
    fn pending_probe_does_not_partially_activate_execution() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let before = device.execution.describe();
            let mut completion = ManualFence::new()?;
            candidate.submit_private_probe(Some(completion.fence()))?;
            let proposal = gate_profile(&candidate, device, crtc, false)?;
            check(matches!(proposal.activate(device), Err(EAGAIN)))?;
            check(device.execution.describe() == before)?;
            candidate.validate()?;
            completion.complete(Ok(()))?;
            let (active, _, after) = proposal.activate(device)?;
            check(after.generation == before.generation + 1)?;
            active.check()
        })
    }

    #[test]
    fn a_superseded_preparation_keeps_its_unpublished_storage() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let mut first = device.execution.prepare(device, Profile::HostV1)?;
            let proposal = gate_profile(&candidate, device, crtc, true)?;
            let mut stale = device.execution.prepare(device, Profile::HostV1)?;
            let proposed = stale.description()?;
            owner
                .access()
                .with_installed_transition(device, |current, locked| {
                    device.execution.publish_proposal(
                        locked,
                        &mut first,
                        proposal.describe().generation,
                        current.configuration(),
                        |contract| current.check_contract(contract),
                    )
                })?;
            candidate.cancel();
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            candidate.validate()?;
            owner
                .access()
                .with_installed_transition(device, |current, locked| {
                    check(
                        device.execution.publish_proposal(
                            locked,
                            &mut stale,
                            proposal.describe().generation,
                            current.configuration(),
                            |contract| current.check_contract(contract),
                        ) == Err(ESTALE),
                    )
                })?;
            check(stale.description()? == proposed)?;
            check(device.execution.describe() == proposed)
        })
    }

    #[test]
    fn another_publication_cannot_consume_identical_prepared_metadata() -> Result {
        let display = CastKms::new(c"castkms-publication-owner")?;
        let other = CastKms::new(c"castkms-publication-other")?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let permission = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(permission.access())?, GFP_KERNEL)?;
            let proposal = gate_profile(&candidate, device, crtc, true)?;
            let mut prepared = device.execution.prepare(device, Profile::HostV1)?;
            let proposed = prepared.description()?;
            with_registered_display(
                &other,
                |other, other_crtc, other_connector, _, other_file| {
                    let owner = owner(&other_file, other_crtc, other_connector)?;
                    let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
                    let proposal = gate_profile(&candidate, other, other_crtc, true)?;
                    let before = other.execution.describe();
                    owner
                        .access()
                        .with_installed_transition(other, |current, locked| {
                            check(
                                other.execution.publish_proposal(
                                    locked,
                                    &mut prepared,
                                    proposal.describe().generation,
                                    current.configuration(),
                                    |contract| current.check_contract(contract),
                                ) == Err(EINVAL),
                            )
                        })?;
                    check(other.execution.describe() == before)
                },
            )?;
            check(prepared.description()? == proposed)?;
            permission
                .access()
                .with_installed_transition(device, |current, locked| {
                    device.execution.publish_proposal(
                        locked,
                        &mut prepared,
                        proposal.describe().generation,
                        current.configuration(),
                        |contract| current.check_contract(contract),
                    )
                })
        })
    }

    #[test]
    fn closing_publication_rejects_prepared_metadata_without_consuming_it() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let proposal = gate_profile(&candidate, device, crtc, true)?;
            let before = device.execution.describe();
            let mut prepared = device.execution.prepare(device, Profile::HostV1)?;
            let proposed = prepared.description()?;
            device.execution.close();
            owner
                .access()
                .with_installed_transition(device, |current, locked| {
                    check(
                        device.execution.publish_proposal(
                            locked,
                            &mut prepared,
                            proposal.describe().generation,
                            current.configuration(),
                            |contract| current.check_contract(contract),
                        ) == Err(ENODEV),
                    )
                })?;
            check(matches!(
                device.execution.prepare(device, Profile::HostV1),
                Err(ENODEV)
            ))?;
            check(prepared.description()? == proposed)?;
            check(device.execution.describe() == before)
        })
    }

    #[test]
    fn a_foreign_blob_device_cannot_change_the_owning_publication() -> Result {
        let display = CastKms::new(c"castkms-prepared-blob-owner")?;
        let other = CastKms::new(c"castkms-prepared-blob-other")?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let permission = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(permission.access())?, GFP_KERNEL)?;
            let proposal = gate_profile(&candidate, device, crtc, true)?;
            let before = device.execution.describe();
            with_registered_display(&other, |other, _, _, _, _| {
                let mut prepared = device.execution.prepare(other, Profile::HostV1)?;
                let proposed = prepared.description()?;
                permission
                    .access()
                    .with_installed_transition(device, |current, locked| {
                        check(
                            device.execution.publish_proposal(
                                locked,
                                &mut prepared,
                                proposal.describe().generation,
                                current.configuration(),
                                |contract| current.check_contract(contract),
                            ) == Err(EINVAL),
                        )
                    })?;
                check(prepared.description()? == proposed)?;
                check(device.execution.describe() == before)
            })
        })
    }
}
