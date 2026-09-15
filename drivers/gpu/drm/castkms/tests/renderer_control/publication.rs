// SPDX-License-Identifier: GPL-2.0-only

//! Prepared metadata is consumed only by its own current publication under renderer control.

use super::*;
use crate::execution::Profile;
use kernel::{
    dma_fence::testing::ManualFence,
    sync::aref::ARef, //
};

#[kunit_tests(rust_castkms_renderer_publication)]
mod cases {
    use super::*;

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
            check(candidate.claim_source(&active, candidate.execution()).err() == Some(ESTALE))?;
            let job = candidate.claim_source(&active, description)?;
            check(job.scene().primary().is_some())?;
            source.seal();
            check(source.prepared()?.is_none())?;
            job.release_without_access();
            check(source.prepared()?.is_some())
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
            let job = candidate.claim_source(&active, description)?;
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
            let job = candidate.claim_source(&active, description)?;
            source.seal();
            job.release_cpu();
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
            let job = candidate.claim_source(&active, description)?;
            let mut completion = ManualFence::new()?;
            let fence = completion.fence();
            source.seal();
            job.release_submitted(&fence);
            let prepared = source.prepared()?.ok_or(EAGAIN)?;
            let retained = prepared.completion()?.ok_or(EINVAL)?;
            check(matches!(retained.status(), kernel::dma_fence::Status::Pending))?;
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
