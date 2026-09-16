// SPDX-License-Identifier: GPL-2.0-only

//! Private backing stays separate from source claims and consumer reuse.

use super::*;
use crate::{
    execution::{
        capabilities::{Format, Profile},
        Description,
    },
    renderer::job::Completion,
    renderer_startup::Active,
};
use kernel::{
    dma_buf::DmaBuf,
    dma_fence::{testing::ManualFence, Status},
    drm::gem::{BaseObject, ExportAccess},
    sync::aref::ARef,
    time::{delay::fsleep, Delta, Instant, Monotonic},
};

pub(super) fn profile() -> Result<Profile> {
    let reference = crate::tests::renderer_proposals::profile()?;
    let mut formats = KVec::new();
    formats.push(
        Format {
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
    Profile::new(*reference.limits(), formats)
}

pub(super) fn activate(
    candidate: &Arc<Candidate>,
    device: &Device<Driver, Registered>,
    crtc: &Crtc<display::Crtc>,
) -> Result<(Active, Description)> {
    candidate.submit_private_probe(None)?;
    let proposal = candidate.propose_profile(profile()?)?;
    device.atomic_update(|transaction| {
        transaction
            .add_crtc_state(crtc)?
            .tag_transition(proposal.describe().transition);
        Ok(())
    })?;
    let (active, _, execution) = proposal.activate(device)?;
    Ok((active, execution))
}

pub(super) fn buffer(device: &Device<Driver>, access: ExportAccess) -> Result<ARef<DmaBuf>> {
    shmem::Object::<gem::Object>::new(
        device,
        640 * 480 * 4,
        Default::default(),
        Default::default(),
    )?
    .export_dma_buf(access)
}

#[kunit_tests(rust_castkms_private_images)]
mod cases {
    use super::*;

    #[test]
    fn retained_host_destinations_cannot_be_registered_as_private_images() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let grantor = super::super::delegated_authority::grant(&file, crtc, connector)?;
            let mut client = crate::capture::client::Client::new(grantor.capture())?;
            let backing = buffer(device, ExportAccess::ReadWrite)?;
            let image = crate::capture::destination::Image::new(
                backing.clone(),
                crate::host_compositor::layout::Layout::new(640, 480)?,
                drm::fourcc::XRGB8888,
                0,
                2560,
                0,
            )?;
            client.register_destination(1, image)?;
            let retained = client.destination(1)?;
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let (active, _) = activate(&candidate, device, crtc)?;
            check(candidate.register_private_image(&active, [640, 480], core::slice::from_ref(&backing)).err() == Some(EEXIST))?;
            client.unregister_destination(1)?;
            check(candidate.register_private_image(&active, [640, 480], core::slice::from_ref(&backing)).err() == Some(EEXIST))?;
            drop(retained);
            // Releasing the tracked role permits recipient registration again, not a
            // claim that externally exposed backing is suitable for private content.
            drop(device.image_storage.register(crate::image_storage::Pool::Recipient, [640, 480], &[backing])?);
            Ok(())
        })
    }

    #[test]
    fn retained_producer_notifications_follow_completed_private_content() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let (active, execution) = activate(&candidate, device, crtc)?;
            let image = candidate.register_private_image(
                &active,
                [640, 480],
                &[buffer(device, ExportAccess::ReadWrite)?],
            )?;
            let (mut scene, configuration) = device
                .output
                .with_accepted(|accepted| {
                    accepted.and_then(|accepted| {
                        Some((accepted.scene?.clone(), accepted.configuration.clone()))
                    })
                })
                .ok_or(EINVAL)?;
            let mut producer = ManualFence::new()?;
            let mut layer = scene.primary().ok_or(EINVAL)?.clone();
            layer.producer = Some(Arc::new(
                kernel::drm::kms::framebuffer::dependencies::Dependencies::acquire(
                    layer.framebuffer(),
                    Some(producer.fence()),
                )?,
                GFP_KERNEL,
            )?);
            scene.set_layer(0, Some(Arc::new(layer, GFP_KERNEL)?));
            device.output.publish_with_configuration(
                kernel::drm::preparation::Source::new(2)?,
                crate::output::SceneUpdate::Replace(Some(scene)),
                configuration,
            );
            let mut job = candidate.claim_render(&active, execution, None, image.prepare(1)?)?;
            job.observe_producers(&device.changed)?;
            let rendered = job.release(Completion::Cpu).ok_or(EINVAL)?;
            check(rendered.content().status() == Status::Pending)?;
            let observer = kernel::sync::poll::testing::Observer::new(device.changed.clone())?;
            producer.complete(Err(EIO))?;
            check(observer.notifications() > 0)?;
            check(rendered.content().status() == Status::Complete(Err(EIO)))?;
            drop(rendered);
            drop(image.prepare(2)?);
            Ok(())
        })
    }

    #[test]
    fn observed_source_admission_does_not_keep_the_worker_alive() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let (active, execution) = activate(&candidate, device, crtc)?;
            let image = candidate.register_private_image(
                &active,
                [640, 480],
                &[buffer(device, ExportAccess::ReadWrite)?],
            )?;
            let observation = active.observation();
            drop(active);
            check(
                candidate
                    .claim_render_observed(&observation, execution, None, image.prepare(1)?)
                    .err()
                    == Some(EIO),
            )?;
            drop(image.prepare(2)?);
            Ok(())
        })
    }

    #[test]
    fn retained_content_excludes_overwrite_without_retaining_source_reads() -> Result {
        with_display(|device, crtc, connector, scanout, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let (active, execution) = activate(&candidate, device, crtc)?;
            let buffer = buffer(device, ExportAccess::ReadWrite)?;
            let image = candidate.register_private_image(&active, [640, 480], &[buffer])?;
            let job = candidate.claim_render(&active, execution, None, image.prepare(1)?)?;
            check(job.destination().dimensions() == [640, 480])?;
            let serial = job.source().scene().content_serial();
            let source = device
                .output
                .with_accepted(|accepted| accepted.map(|item| ARef::from(item.source)))
                .ok_or(EINVAL)?;
            let rendered = job.release(Completion::Cpu).ok_or(EINVAL)?;
            source.seal();
            check(source.prepared()?.is_some())?;
            check(image.prepare(2).err() == Some(EBUSY))?;
            device.atomic_update(|transaction| transaction.set_crtc_config(crtc, Some(scanout)))?;
            check(rendered.content().content_serial() == serial)?;
            check(rendered.image().dimensions() == [640, 480])?;
            candidate.with_content(&active, rendered.content(), || Ok(()))?;
            drop(rendered);
            drop(image.prepare(2)?);
            check(image.prepare(2).err() == Some(ESTALE))
        })
    }

    #[test]
    fn submitted_write_retains_removed_registration_until_native_completion() -> Result {
        for result in [Ok(()), Err(EIO)] {
            with_display(|device, crtc, connector, _, file| {
                let owner = owner(&file, crtc, connector)?;
                let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
                let (active, execution) = activate(&candidate, device, crtc)?;
                let buffer = buffer(device, ExportAccess::ReadWrite)?;
                let image =
                    candidate.register_private_image(&active, [640, 480], &[buffer.clone()])?;
                let mut completion = ManualFence::new()?;
                let job = candidate.claim_render(&active, execution, None, image.prepare(1)?)?;
                let rendered = job
                    .release(Completion::Submitted(completion.fence()))
                    .ok_or(EINVAL)?;
                check(rendered.content().status() == Status::Pending)?;
                drop(rendered);
                drop(image);
                check(
                    candidate
                        .register_private_image(&active, [640, 480], &[buffer.clone()])
                        .err()
                        == Some(EEXIST),
                )?;
                completion.complete(result)?;
                let start = Instant::<Monotonic>::now();
                loop {
                    match candidate.register_private_image(&active, [640, 480], &[buffer.clone()]) {
                        Ok(_) => break,
                        Err(EEXIST) if start.elapsed() < Delta::from_millis(2000) => {
                            fsleep(Delta::from_millis(1))
                        }
                        Err(error) => return Err(error),
                    }
                }
                Ok(())
            })?;
        }
        Ok(())
    }

    #[test]
    fn no_access_release_returns_private_capacity_without_content() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let (active, execution) = activate(&candidate, device, crtc)?;
            let image = candidate.register_private_image(
                &active,
                [640, 480],
                &[buffer(device, ExportAccess::ReadWrite)?],
            )?;
            let job = candidate.claim_render(&active, execution, None, image.prepare(1)?)?;
            check(job.release(Completion::WithoutAccess).is_none())?;
            drop(image.prepare(2)?);
            Ok(())
        })
    }

    #[test]
    fn registration_rejects_source_aliases_and_readonly_storage() -> Result {
        with_display(|device, crtc, connector, scanout, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let (active, _) = activate(&candidate, device, crtc)?;
            let source = scanout
                .framebuffer
                .object_at(0)?
                .export_dma_buf(ExportAccess::ReadWrite)?;
            check(
                candidate
                    .register_private_image(&active, [640, 480], &[source])
                    .err()
                    == Some(EINVAL),
            )?;
            let readonly = buffer(device, ExportAccess::ReadOnly)?;
            check(
                candidate
                    .register_private_image(&active, [640, 480], &[readonly])
                    .err()
                    == Some(EACCES),
            )?;
            let writable = buffer(device, ExportAccess::ReadWrite)?;
            check(
                candidate
                    .register_private_image(
                        &active,
                        [640, 480],
                        &[writable.clone(), writable.clone()],
                    )
                    .err()
                    == Some(EINVAL),
            )?;
            check(
                candidate
                    .register_private_image(&active, [0, 480], &[writable.clone()])
                    .err()
                    == Some(EINVAL),
            )?;
            check(
                candidate
                    .register_private_image(&active, [640, 480], &[])
                    .err()
                    == Some(EINVAL),
            )?;
            let image =
                candidate.register_private_image(&active, [640, 480], &[writable.clone()])?;
            check(
                candidate
                    .register_private_image(&active, [640, 480], &[writable.clone()])
                    .err()
                    == Some(EEXIST),
            )?;
            drop(image);
            drop(candidate.register_private_image(&active, [640, 480], &[writable])?);
            Ok(())
        })
    }

    #[test]
    fn denied_source_admission_releases_the_private_reservation() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let (active, execution) = activate(&candidate, device, crtc)?;
            let image = candidate.register_private_image(
                &active,
                [640, 480],
                &[buffer(device, ExportAccess::ReadWrite)?],
            )?;
            let prepared = image.prepare(1)?;
            owner.revoke();
            check(
                candidate
                    .claim_render(&active, execution, None, prepared)
                    .err()
                    == Some(EKEYREVOKED),
            )?;
            drop(image.prepare(2)?);
            Ok(())
        })
    }

    #[test]
    fn replacement_renderer_cannot_claim_another_incarnations_private_image() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let first = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let (active, _) = activate(&first, device, crtc)?;
            let image = first.register_private_image(
                &active,
                [640, 480],
                &[buffer(device, ExportAccess::ReadWrite)?],
            )?;
            let second = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let (next, execution) = activate(&second, device, crtc)?;
            check(
                second
                    .claim_render(&next, execution, None, image.prepare(1)?)
                    .err()
                    == Some(EACCES),
            )?;
            drop(image.prepare(2)?);
            Ok(())
        })
    }

    #[test]
    fn private_use_names_never_wrap_or_become_retryable_after_reservation() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let (active, _) = activate(&candidate, device, crtc)?;
            let image = candidate.register_private_image(
                &active,
                [640, 480],
                &[buffer(device, ExportAccess::ReadWrite)?],
            )?;
            check(image.prepare(0).err() == Some(EINVAL))?;
            drop(image.prepare(u64::MAX)?);
            check(image.prepare(1).err() == Some(EOVERFLOW))
        })
    }

    #[test]
    fn registrations_are_bounded_across_private_handles() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let (active, _) = activate(&candidate, device, crtc)?;
            let mut images = KVec::new();
            for _ in 0..128 {
                images.push(
                    candidate.register_private_image(
                        &active,
                        [640, 480],
                        &[buffer(device, ExportAccess::ReadWrite)?],
                    )?,
                    GFP_KERNEL,
                )?;
            }
            let next = buffer(device, ExportAccess::ReadWrite)?;
            check(
                candidate
                    .register_private_image(&active, [640, 480], &[next.clone()])
                    .err()
                    == Some(EBUSY),
            )?;
            drop(images.pop());
            drop(candidate.register_private_image(&active, [640, 480], &[next])?);
            Ok(())
        })
    }
}
