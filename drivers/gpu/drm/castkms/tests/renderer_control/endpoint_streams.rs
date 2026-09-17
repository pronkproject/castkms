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
            endpoint.publish(|_| Ok(()))?;
            check(!endpoint.source_readable()?)?;
            check(endpoint.begin_source(1).err() == Some(ESTALE))?;
            let entry = device.constraints_output(crtc)?.lookup(endpoint.constraints_id()?)?;
            device.atomic_update(|state| state.add_crtc_state(crtc)?.set_constraints(&entry))?;
            check(endpoint.source_readable()?)?;
            let pending = endpoint.begin_source(1)?;
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
    fn issuer_revocation_still_accepts_outstanding_native_completion() -> Result {
        let display = CastKms::new_constraints(c"castkms-endpoint-stream-retire", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let endpoint = endpoints::prepared(device, &owner)?;
            endpoint.publish(|_| Ok(()))?;
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
            endpoint.publish(|_| Ok(()))?;
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
            endpoint.publish(|_| Ok(()))?;
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
}
