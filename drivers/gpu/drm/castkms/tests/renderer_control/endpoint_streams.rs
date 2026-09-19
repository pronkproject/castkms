// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use crate::renderer::job::Completion;
use kernel::dma_fence::{testing::ManualFence, Status};

#[kunit_tests(rust_castkms_endpoint_streams)]
mod cases {
    use super::*;

    #[test]
    fn changed_scenes_use_one_job_and_retry_unpublished_claims() -> Result {
        let display = CastKms::new_constraints(c"castkms-endpoint-stream", 1)?;
        with_registered_display(&display, |device, crtc, connector, scanout, file| {
            let owner = owner(&file, crtc, connector)?;
            let endpoint = endpoints::prepared(device, &owner)?;
            check(!endpoint.source_readable()?)?;
            check(endpoint.begin_source(1).err() == Some(ENODATA))?;
            endpoint.publish(None, |_| Ok(()))?;
            check(!endpoint.source_readable()?)?;
            check(endpoint.begin_source(1).err() == Some(ESTALE))?;
            let entry = device.constraints_output(crtc)?.lookup(endpoint.constraints_id()?)?;
            device.atomic_update(|state| state.add_crtc_state(crtc)?.set_constraints(&entry))?;
            check(endpoint.source_readable()?)?;
            let mut pending = endpoint.begin_source(1)?;
            check(pending.constraints_id() == entry.id())?;
            check(!endpoint.source_readable()?)?;
            let first = pending.id();
            check(pending.scene_description()?.output == [640, 480])?;
            check(pending.producer_completion()?.is_none())?;
            check(endpoint.begin_source(1).err() == Some(EBUSY))?;
            drop(pending);
            check(endpoint.source_readable()?)?;
            let retry = endpoint.begin_source(1)?;
            check(retry.id() > first)?;
            let id = retry.id();
            retry.publish(|| ())?;
            check(endpoint.begin_source(1).err() == Some(EBUSY))?;
            check(endpoint.release_source(id + 1, Completion::WithoutAccess) == Err(ENOENT))?;
            endpoint.release_source(id, Completion::WithoutAccess)?;
            endpoint.release_source(id, Completion::WithoutAccess)?;
            let mut previous = 0;
            for _ in 0..12 {
                let pending = endpoint.begin_source(1)?;
                let serial = pending.scene_description()?.content_serial;
                check(serial > previous)?;
                previous = serial;
                let id = pending.id();
                pending.publish(|| ())?;
                endpoint.release_source(id, Completion::Cpu)?;
                check(endpoint.completed_image(1)?.content().content_serial()
                    .is_some_and(|content| content.get() == serial))?;
                check(endpoint.begin_source(1).err() == Some(ENODATA))?;
                device.atomic_update(|state| state.set_crtc_config(crtc, Some(scanout)))?;
            }
            Ok(())
        })
    }

    #[test]
    fn active_blanking_acquires_a_zero_plane_job() -> Result {
        let display = CastKms::new_constraints(c"castkms-endpoint-blank-job", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let endpoint = endpoints::prepared(device, &owner)?;
            endpoint.publish(None, |_| Ok(()))?;
            let entry = device.constraints_output(crtc)?.lookup(endpoint.constraints_id()?)?;
            device.atomic_update(|state| state.add_crtc_state(crtc)?.set_constraints(&entry))?;

            let first = endpoint.begin_source(1)?;
            let first_serial = first.scene_description()?.content_serial;
            check(first.scene_description()?.layers.len() == 1)?;
            let first_id = first.id();
            first.publish(|| ())?;
            endpoint.release_source(first_id, Completion::Cpu)?;
            device.atomic_update(|state| state.disable_plane(crtc.primary_plane()))?;
            check(endpoint.source_readable()?)?;

            let blank = endpoint.begin_source(1)?;
            {
                let description = blank.scene_description()?;
                check(description.layers.is_empty())?;
                check(description.output == [640, 480])?;
                check(description.content_serial > first_serial)?;
            }
            let blank_id = blank.id();
            blank.publish(|| ())?;
            endpoint.release_source(blank_id, Completion::Cpu)?;
            check(endpoint.completed_image(1).is_ok())?;
            check(endpoint.begin_source(1).err() == Some(ENODATA))?;
            Ok(())
        })
    }

    #[test]
    fn active_blanking_rejects_the_prior_private_image() -> Result {
        let display = CastKms::new_constraints(c"castkms-endpoint-blank-image", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let endpoint = endpoints::prepared(device, &owner)?;
            endpoint.publish(None, |_| Ok(()))?;
            let entry = device.constraints_output(crtc)?.lookup(endpoint.constraints_id()?)?;
            device.atomic_update(|state| state.add_crtc_state(crtc)?.set_constraints(&entry))?;
            let source = endpoint.begin_source(1)?;
            let id = source.id();
            source.publish(|| ())?;
            endpoint.release_source(id, Completion::Cpu)?;
            check(endpoint.completed_image(1).is_ok())?;

            device.atomic_update(|state| state.disable_plane(crtc.primary_plane()))?;
            check(endpoint.completed_image(1).err() == Some(ESTALE))?;
            check(endpoint.begin_output(1).err() == Some(ESTALE))
        })
    }

    #[test]
    fn terminal_producer_errors_never_become_source_readiness() -> Result {
        let display = CastKms::new_constraints(c"castkms-endpoint-producer-error", 1)?;
        with_registered_display(&display, |device, crtc, connector, scanout, file| {
            let owner = owner(&file, crtc, connector)?;
            let endpoint = endpoints::prepared(device, &owner)?;
            endpoint.publish(None, |_| Ok(()))?;
            let entry = device.constraints_output(crtc)?.lookup(endpoint.constraints_id()?)?;
            for error in [EAGAIN, EBUSY, ENODATA, EIO] {
                let mut producer = ManualFence::new()?;
                producer.complete(Err(error))?;
                device.atomic_update(|mut state| {
                    state.as_mut().set_crtc_config(crtc, Some(scanout))?;
                    state.add_crtc_state(crtc)?.set_constraints(&entry)?;
                    state.add_plane_state(crtc.primary_plane())?
                        .set_producer_fence(Some(producer.fence()));
                    Ok(())
                })?;
                let mut pending = endpoint.begin_source(1)?;
                check(pending.producer_completion().err() == Some(EREMOTEIO))?;
                let mut installed = false;
                check(pending.publish(|| installed = true) == Err(EREMOTEIO))?;
                check(!installed && !endpoint.source_readable()?)?;
                check(endpoint.begin_source(1).err() == Some(ENODATA))?;
                check(producer.fence().status() == Status::Complete(Err(error)))?;
            }
            device.atomic_update(|state| state.set_crtc_config(crtc, Some(scanout)))?;
            check(endpoint.begin_source(1)?.producer_completion()?.is_none())?;
            Ok(())
        })
    }

    #[test]
    fn issuer_revocation_still_accepts_outstanding_native_completion() -> Result {
        let display = CastKms::new_constraints(c"castkms-endpoint-stream-retire", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let endpoint = endpoints::prepared(device, &owner)?;
            endpoint.publish(None, |_| Ok(()))?;
            let entry = device.constraints_output(crtc)?.lookup(endpoint.constraints_id()?)?;
            device.atomic_update(|state| state.add_crtc_state(crtc)?.set_constraints(&entry))?;
            let pending = endpoint.begin_source(1)?;
            let id = pending.id();
            pending.publish(|| ())?;
            let hold = crtc.display.output.with_accepted(|accepted| {
                accepted.ok_or(EINVAL)?.source.hold_admission()
            })?;
            owner.revoke();
            check(endpoint.unregister_image(1) == Err(EBUSY))?;
            check(hold.prepared()?.is_none())?;
            let mut native = ManualFence::new()?;
            endpoint.release_source(id, Completion::Submitted(native.fence()))?;
            let prepared = hold.prepared()?.ok_or(EINVAL)?;
            let completion = prepared.completion()?.ok_or(EINVAL)?;
            check(completion.status() == Status::Pending)?;
            check(endpoint.completed_image(1).err() == Some(EKEYREVOKED))?;
            endpoint.unregister_image(1)?;
            check(completion.status() == Status::Pending)?;
            native.complete(Err(EIO))?;
            check(completion.status() == Status::Complete(Err(EIO)))?;
            Ok(())
        })
    }

    #[test]
    fn close_before_source_publication_installs_nothing_and_releases_without_access() -> Result {
        let display = CastKms::new_constraints(c"castkms-endpoint-stream-close", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let endpoint = endpoints::prepared(device, &owner)?;
            endpoint.publish(None, |_| Ok(()))?;
            let entry = device.constraints_output(crtc)?.lookup(endpoint.constraints_id()?)?;
            device.atomic_update(|state| state.add_crtc_state(crtc)?.set_constraints(&entry))?;
            let pending = endpoint.begin_source(1)?;
            let hold = crtc.display.output.with_accepted(|accepted| {
                accepted.ok_or(EINVAL)?.source.hold_admission()
            })?;
            endpoint.close();
            check(hold.prepared()?.is_none())?;
            let mut installed = false;
            check(pending.publish(|| installed = true) == Err(EKEYREVOKED))?;
            check(!installed && hold.prepared()?.is_some())?;
            check(endpoint.begin_source(1).err() == Some(EKEYREVOKED))?;
            Ok(())
        })
    }

    #[test]
    fn revoked_pending_publication_can_never_install_source_files() -> Result {
        let display = CastKms::new_constraints(c"castkms-endpoint-stream-authority", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let endpoint = endpoints::prepared(device, &owner)?;
            endpoint.publish(None, |_| Ok(()))?;
            let entry = device.constraints_output(crtc)?.lookup(endpoint.constraints_id()?)?;
            device.atomic_update(|state| state.add_crtc_state(crtc)?.set_constraints(&entry))?;
            let pending = endpoint.begin_source(1)?;
            let hold = crtc.display.output.with_accepted(|accepted| {
                accepted.ok_or(EINVAL)?.source.hold_admission()
            })?;
            owner.revoke();
            let mut installed = false;
            check(pending.publish(|| installed = true) == Err(EKEYREVOKED))?;
            check(!installed && hold.prepared()?.is_some())?;
            endpoint.unregister_image(1)?;
            Ok(())
        })
    }

    #[test]
    fn master_reacquisition_drains_old_claim_before_endpoint_reuse() -> Result {
        let display = CastKms::new_constraints(c"castkms-endpoint-stream-generation", 1)?;
        with_registered_display(&display, |device, crtc, connector, scanout, file| {
            let owner = owner(&file, crtc, connector)?;
            let endpoint = endpoints::prepared(device, &owner)?;
            endpoint.publish(None, |_| Ok(()))?;
            let entry = device.constraints_output(crtc)?.lookup(endpoint.constraints_id()?)?;
            device.atomic_update(|state| state.add_crtc_state(crtc)?.set_constraints(&entry))?;
            let pending = endpoint.begin_source(1)?;
            let id = pending.id();
            pending.publish(|| ())?;
            let master = file.file().master_snapshot().ok_or(EINVAL)?;

            <Driver as kernel::drm::Driver>::master_changed(device, None);
            <Driver as kernel::drm::Driver>::master_changed(
                device,
                Some(master.master().clone()),
            );
            check(endpoint.describe()?.phase == crate::renderer::endpoint::Phase::Withdrawn)?;
            check(!endpoint.source_readable()?)?;
            check(!endpoint.output_readable()?)?;
            check(endpoint.begin_source(1).err() == Some(EBUSY))?;
            endpoint.release_source(id, Completion::WithoutAccess)?;
            check(endpoint.describe()?.phase == crate::renderer::endpoint::Phase::Empty)?;
            device.atomic_update(|state| state.set_crtc_config(crtc, None))?;
            device.constraints_output(crtc)?.restore_default()?;
            endpoint.declare(private_images::profile()?, [640, 480])?;
            endpoint.register_image(2, [640, 480], &[
                private_images::buffer(device, kernel::drm::gem::ExportAccess::ReadWrite)?,
            ])?;
            endpoint.publish(None, |_| Ok(()))?;
            let entry = device.constraints_output(crtc)?.lookup(endpoint.constraints_id()?)?;
            device.atomic_update(|mut state| {
                state.as_mut().set_crtc_config(crtc, Some(scanout))?;
                state.add_crtc_state(crtc)?.set_constraints(&entry)
            })?;
            let pending = endpoint.begin_source(2)?;
            let current = pending.id();
            check(current > id)?;
            pending.publish(|| ())?;
            check(endpoint.release_source(id, Completion::WithoutAccess) == Err(ENOENT))?;
            endpoint.release_source(current, Completion::WithoutAccess)
        })
    }
}
