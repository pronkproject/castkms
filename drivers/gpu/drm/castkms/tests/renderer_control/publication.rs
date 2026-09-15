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

    #[test]
    fn replacing_a_negotiated_worker_retains_its_outstanding_source_read() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let first = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            first.submit_private_probe(None)?;
            let proposal = first.propose_profile(linear_profile()?)?;
            device.atomic_update(|mut transaction| {
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
            device.atomic_update(|mut transaction| {
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
    fn pending_capabilities_cannot_use_probe_only_activation() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            candidate.submit_private_probe(None)?;
            let before = device.execution.describe();
            let proposal =
                candidate.propose_profile(crate::tests::renderer_proposals::profile()?)?;
            check(candidate.activate(device).err() == Some(EAGAIN))?;
            check(device.execution.describe() == before)?;
            proposal.validate()?;
            proposal.cancel();
            let (_active, _, next) = candidate.activate(device)?;
            check(next.generation == before.generation + 1)
        })
    }

    #[test]
    fn publishing_metadata_invalidates_the_old_candidate() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Candidate::begin(owner.access())?;
            let before = device.execution.describe();
            let mut prepared = device.execution.prepare(device, Profile::HostV1)?;
            let next = prepared.description()?;
            check(next.generation == before.generation + 1)?;
            check(device.execution.describe() == before)?;
            candidate.with_activation_control(device, |_, locked| {
                device.execution.publish(locked, &mut prepared)
            })?;
            check(device.execution.describe() == next)?;
            check(prepared.description() == Err(EALREADY))?;
            check(candidate.validate() == Err(ESTALE))?;
            candidate.cancel();
            let replacement = Candidate::begin(owner.access())?;
            replacement.with_activation_control(device, |_, locked| {
                check(device.execution.publish(locked, &mut prepared) == Err(EALREADY))
            })
        })
    }

    #[test]
    fn gpu_publication_closes_new_host_source_admission() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Candidate::begin(owner.access())?;
            let host = device.host.configure(
                device,
                crate::host_compositor::layout::Layout::new(640, 480)?,
            )?;
            let mut prepared = device.execution.prepare(device, Profile::GpuV1)?;
            let description = prepared.description()?;
            candidate.with_activation_control(device, |_, locked| {
                device.execution.publish(locked, &mut prepared)
            })?;
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
            let candidate = Candidate::begin(owner.access())?;
            let before = device.execution.describe();
            candidate.submit_private_probe(None)?;
            let (active, _source, description) = candidate.activate(device)?;
            check(description.generation == before.generation + 1)?;
            check(description.profile == Profile::GpuV1)?;
            check(description == device.execution.describe())?;
            active.check()?;
            check(matches!(device.startup.begin(), Err(EBUSY)))?;
            drop(active);
            check(matches!(device.startup.begin(), Err(EBUSY)))
        })
    }

    #[test]
    fn active_renderer_claim_retires_only_after_explicit_release() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Candidate::begin(owner.access())?;
            candidate.submit_private_probe(None)?;
            let (active, _, description) = candidate.activate(device)?;
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
            let description = pending.description()?;
            check(description.format == drm::fourcc::XRGB8888)?;
            check(description.modifier.is_none())?;
            check(description.dimensions == [640, 480])?;
            check(description.source == [0, 0, 640 << 16, 480 << 16])?;
            check(description.destination == [640, 480])?;
            check(description.output == [640, 480])?;
            check(description.plane_count == 1)?;
            check(description.content_serial != 0)?;
            let plane = pending.plane(0)?;
            check(plane.pitch == 2560)?;
            check(plane.offset == 0)?;
            let buffer = plane.export()?;
            check(!buffer.is_writable())?;
            check(session.begin_source().err() == Some(EBUSY))?;
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
            let candidate = Candidate::begin(owner.access())?;
            candidate.submit_private_probe(None)?;
            let (active, _, description) = candidate.activate(device)?;
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
            let candidate = Candidate::begin(owner.access())?;
            candidate.submit_private_probe(None)?;
            let (active, _, description) = candidate.activate(device)?;
            let source = device
                .output
                .with_accepted(|accepted| accepted.map(|item| ARef::from(item.source)))
                .ok_or(EINVAL)?;
            let job = candidate.claim_source(&active, description, None)?;
            source.seal();
            job.release(Completion::Cpu);
            let prepared = source.prepared()?.ok_or(EAGAIN)?;
            check(prepared.completion()?.is_none())
        })
    }

    #[test]
    fn submitted_renderer_release_transfers_native_completion() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Candidate::begin(owner.access())?;
            candidate.submit_private_probe(None)?;
            let (active, _, description) = candidate.activate(device)?;
            let source = device
                .output
                .with_accepted(|accepted| accepted.map(|item| ARef::from(item.source)))
                .ok_or(EINVAL)?;
            let job = candidate.claim_source(&active, description, None)?;
            let mut completion = ManualFence::new()?;
            let fence = completion.fence();
            source.seal();
            job.release(Completion::Submitted(fence));
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
            let candidate = Candidate::begin(owner.access())?;
            let before = device.execution.describe();
            let mut completion = ManualFence::new()?;
            candidate.submit_private_probe(Some(completion.fence()))?;
            check(matches!(candidate.activate(device), Err(EAGAIN)))?;
            check(device.execution.describe() == before)?;
            candidate.validate()?;
            completion.complete(Ok(()))?;
            let (active, _, after) = candidate.activate(device)?;
            check(after.generation == before.generation + 1)?;
            active.check()
        })
    }

    #[test]
    fn a_superseded_preparation_keeps_its_unpublished_storage() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Candidate::begin(owner.access())?;
            let mut first = device.execution.prepare(device, Profile::HostV1)?;
            let mut stale = device.execution.prepare(device, Profile::HostV1)?;
            let proposed = stale.description()?;
            candidate.with_activation_control(device, |_, locked| {
                device.execution.publish(locked, &mut first)
            })?;
            candidate.cancel();
            let candidate = Candidate::begin(owner.access())?;
            candidate.with_activation_control(device, |_, locked| {
                check(device.execution.publish(locked, &mut stale) == Err(ESTALE))
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
            let candidate = Candidate::begin(permission.access())?;
            let mut prepared = device.execution.prepare(device, Profile::HostV1)?;
            let proposed = prepared.description()?;
            with_registered_display(
                &other,
                |other, other_crtc, other_connector, _, other_file| {
                    let owner = owner(&other_file, other_crtc, other_connector)?;
                    let candidate = Candidate::begin(owner.access())?;
                    let before = other.execution.describe();
                    candidate.with_activation_control(other, |_, locked| {
                        check(other.execution.publish(locked, &mut prepared) == Err(EINVAL))
                    })?;
                    check(other.execution.describe() == before)
                },
            )?;
            check(prepared.description()? == proposed)?;
            candidate.with_activation_control(device, |_, locked| {
                device.execution.publish(locked, &mut prepared)
            })
        })
    }

    #[test]
    fn closing_publication_rejects_prepared_metadata_without_consuming_it() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Candidate::begin(owner.access())?;
            let before = device.execution.describe();
            let mut prepared = device.execution.prepare(device, Profile::HostV1)?;
            let proposed = prepared.description()?;
            device.execution.close();
            candidate.with_activation_control(device, |_, locked| {
                check(device.execution.publish(locked, &mut prepared) == Err(ENODEV))
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
            let candidate = Candidate::begin(permission.access())?;
            let before = device.execution.describe();
            with_registered_display(&other, |other, _, _, _, _| {
                let mut prepared = device.execution.prepare(other, Profile::HostV1)?;
                let proposed = prepared.description()?;
                candidate.with_activation_control(device, |_, locked| {
                    check(device.execution.publish(locked, &mut prepared) == Err(EINVAL))
                })?;
                check(prepared.description()? == proposed)?;
                check(device.execution.describe() == before)
            })
        })
    }
}
