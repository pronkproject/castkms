// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use crate::renderer::endpoint::{Endpoint, Phase};
use kernel::drm::gem::ExportAccess;
use kernel::workqueue::{self, impl_has_work, new_work, Work, WorkItem};

#[pin_data]
struct Closer {
    endpoint: Arc<Endpoint>,
    #[pin]
    work: Work<Self>,
}

impl_has_work! {
    impl HasWork<Self> for Closer { self.work }
}

impl WorkItem for Closer {
    type Pointer = Arc<Self>;

    fn run(closer: Arc<Self>) {
        closer.endpoint.close();
    }
}

pub(super) fn prepared(
    device: &Device<Driver, Registered>,
    owner: &Owner,
) -> Result<Arc<Endpoint>> {
    let endpoint = Endpoint::new(owner.access(), device.to_registered_ref())?;
    endpoint.declare(private_images::profile()?, [640, 480])?;
    endpoint.register_image(1, [640, 480], &[
        private_images::buffer(device, ExportAccess::ReadWrite)?,
    ])?;
    Ok(endpoint)
}

#[kunit_tests(rust_castkms_renderer_endpoints)]
mod cases {
    use super::*;

    #[test]
    fn reply_copyout_can_reenter_endpoint_and_authority() -> Result {
        let display = CastKms::new_constraints(c"castkms-endpoint-reply-reentry", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let endpoint = prepared(device, &owner)?;
            let mut copied_id = 0;
            endpoint.publish(None, |id| {
                copied_id = id;
                check(endpoint.describe()?.phase == Phase::Publishing)?;
                owner.access().with_output(|| Ok(()))
            })?;
            check(copied_id != 0 && endpoint.constraints_id()? == copied_id)
        })
    }

    #[test]
    fn close_during_reply_cannot_publish_a_backend() -> Result {
        let display = CastKms::new_constraints(c"castkms-endpoint-reply-close", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let endpoint = prepared(device, &owner)?;
            let output = device.constraints_output(crtc)?;
            let generation = output.snapshot(0)?.info().generation;
            let mut copied_id = 0;
            check(endpoint.publish(None, |id| {
                copied_id = id;
                endpoint.close();
                Ok(())
            }) == Err(EKEYREVOKED))?;
            check(copied_id != 0)?;
            check(output.snapshot(0)?.info().generation == generation)?;
            check(output.lookup(copied_id).err() == Some(ESTALE))
        })
    }

    #[test]
    fn issuer_revocation_during_reply_cannot_publish_a_backend() -> Result {
        let display = CastKms::new_constraints(c"castkms-endpoint-reply-revoke", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let endpoint = prepared(device, &owner)?;
            let output = device.constraints_output(crtc)?;
            let generation = output.snapshot(0)?.info().generation;
            let mut copied_id = 0;
            check(endpoint.publish(None, |id| {
                copied_id = id;
                owner.revoke();
                Ok(())
            }) == Err(EKEYREVOKED))?;
            check(copied_id != 0)?;
            check(output.snapshot(0)?.info().generation == generation)?;
            check(output.lookup(copied_id).err() == Some(ESTALE))
        })
    }

    #[test]
    fn racing_close_cannot_leave_a_selectable_unowned_backend() -> Result {
        let display = CastKms::new_constraints(c"castkms-endpoint-close-race", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let output = device.constraints_output(crtc)?;
            for _ in 0..32 {
                let endpoint = prepared(device, &owner)?;
                let closing = endpoint.clone();
                let closer = Arc::pin_init(pin_init!(Closer {
                    endpoint: closing,
                    work <- new_work!("castkms-endpoint-close-test"),
                }), GFP_KERNEL)?;
                let queued = workqueue::system_dfl().enqueue(closer.clone()).is_ok();
                let result = endpoint.publish(None, |_| Ok(()));
                closer.work.flush();
                check(queued && (result.is_ok() || result == Err(EKEYREVOKED)))?;
                check(endpoint.constraints_id() == Err(EKEYREVOKED))?;
                check(output.snapshot(0)?.entries().all(|entry| {
                    entry.entry.id() == output.default_entry().id() || !entry.selectable
                }))?;
            }
            Ok(())
        })
    }

    #[test]
    fn failed_publication_restores_configuration_without_leaking_registration_pins() -> Result {
        let display = CastKms::new_constraints(c"castkms-endpoint-retry", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let endpoint = Endpoint::new(owner.access(), device.to_registered_ref())?;
            check(endpoint.constraints_id()? == 0)?;
            check(endpoint.publish(None, |_| Ok(())) == Err(ENODATA))?;
            endpoint.declare(private_images::profile()?, [640, 480])?;
            check(endpoint.publish(None, |_| Ok(())) == Err(ENODATA))?;
            let buffer = private_images::buffer(device, ExportAccess::ReadWrite)?;
            endpoint.register_image(1, [640, 480], &[buffer.clone()])?;
            let output = device.constraints_output(crtc)?;
            let generation = output.snapshot(0)?.info().generation;
            check(endpoint.publish(None, |_| Err(EFAULT)) == Err(EFAULT))?;
            check(output.snapshot(0)?.info().generation == generation)?;
            check(endpoint.constraints_id()? == 0)?;
            endpoint.unregister_image(1)?;
            endpoint.register_image(2, [640, 480], &[buffer.clone()])?;
            let mut id = 0;
            endpoint.publish(None, |value| { id = value; Ok(()) })?;
            check(id != 0 && endpoint.constraints_id()? == id)?;
            check(endpoint.unregister_image(2) == Err(EBUSY))?;
            check(endpoint.register_image(3, [640, 480], &[buffer]) == Err(EBUSY))?;
            check(endpoint.publish(None, |_| Ok(())) == Err(EALREADY))?;
            check(endpoint.declare(private_images::profile()?, [640, 480]) == Err(EALREADY))?;
            check(output.lookup(id).is_ok())?;
            drop(endpoint);
            check(output.lookup(id).err() == Some(ESTALE))?;
            check(output.snapshot(0)?.info().selected_id == output.default_entry().id())?;
            Ok(())
        })
    }

    #[test]
    fn closed_selected_endpoint_can_be_disabled() -> Result {
        let display = CastKms::new_constraints(c"castkms-endpoint-disable", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let endpoint = prepared(device, &owner)?;
            endpoint.publish(None, |_| Ok(()))?;
            let output = device.constraints_output(crtc)?;
            let entry = output.lookup(endpoint.constraints_id()?)?;
            device.atomic_update(|state| state.add_crtc_state(crtc)?.set_constraints(&entry))?;
            endpoint.close();
            check(crtc.display.constraints.as_ref().ok_or(EINVAL)?
                .resolve(&entry).err() == Some(ESTALE))?;
            device.atomic_update(|state| state.set_crtc_config(crtc, None))?;
            check(output.selected().id() == entry.id())?;
            output.restore_default()?;
            output.forget(entry.id())?;
            Ok(())
        })
    }

    #[test]
    fn failed_readiness_never_lists_an_backend_and_close_is_terminal() -> Result {
        let display = CastKms::new_constraints(c"castkms-endpoint-readiness", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let endpoint = Endpoint::new(owner.access(), device.to_registered_ref())?;
            endpoint.declare(private_images::profile()?, [640, 480])?;
            endpoint.register_image(1, [640, 480], &[
                private_images::buffer(device, ExportAccess::ReadWrite)?,
            ])?;
            let mut fence = kernel::dma_fence::testing::ManualFence::new()?;
            check(endpoint.publish(Some(fence.fence()), |_| Ok(())) == Err(EBUSY))?;
            fence.complete(Err(EAGAIN))?;
            check(endpoint.publish(Some(fence.fence()), |_| Ok(())) == Err(EREMOTEIO))?;
            endpoint.unregister_image(1)?;
            check(device.constraints_output(crtc)?.snapshot(0)?.info().count == 1)?;
            endpoint.close();
            check(endpoint.constraints_id() == Err(EKEYREVOKED))?;
            check(endpoint.publish(None, |_| Ok(())) == Err(EKEYREVOKED))?;
            check(endpoint.declare(private_images::profile()?, [640, 480]) == Err(EKEYREVOKED))?;
            check(endpoint.unregister_image(1) == Err(EKEYREVOKED))?;
            endpoint.close();
            Ok(())
        })
    }

    #[test]
    fn replacement_preserves_owner_scope() -> Result {
        let display = CastKms::new_constraints(c"castkms-endpoint-replace", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let first = prepared(device, &owner)?;
            let second = prepared(device, &owner)?;
            first.publish(None, |_| Ok(()))?;
            second.publish(None, |_| Ok(()))?;
            let output = device.constraints_output(crtc)?;
            let entry = output.lookup(first.constraints_id()?)?;
            device.atomic_update(|state| state.add_crtc_state(crtc)?.set_constraints(&entry))?;
            drop(first);
            check(output.snapshot(0)?.info().selected_id == entry.id())?;
            check(output.snapshot(0)?.entries()
                .any(|item| item.entry.id() == entry.id() && !item.selectable))?;
            // Selected withdrawn metadata remains resolvable for unchanged shutdown.
            check(output.lookup(entry.id()).is_ok())?;
            check(output.lookup(second.constraints_id()?).is_ok())?;
            owner.revoke();
            check(second.constraints_id() == Err(EKEYREVOKED))?;
            second.unregister_image(1)?;
            Ok(())
        })
    }

    #[test]
    fn same_master_reacquisition_reuses_endpoint_with_a_fresh_generation() -> Result {
        let display = CastKms::new_constraints(c"castkms-endpoint-reactivate", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let endpoint = prepared(device, &owner)?;
            endpoint.publish(None, |_| Ok(()))?;
            let old = endpoint.constraints_id()?;
            let master = file.file().master_snapshot().ok_or(EINVAL)?;

            <Driver as kernel::drm::Driver>::master_changed(device, None);
            check(endpoint.describe().err() == Some(EACCES))?;
            <Driver as kernel::drm::Driver>::master_changed(
                device,
                Some(master.master().clone()),
            );

            check(endpoint.describe()?.phase == crate::renderer::endpoint::Phase::Empty)?;
            check(device.constraints_output(crtc)?.lookup(old).err() == Some(ESTALE))?;
            endpoint.declare(private_images::profile()?, [640, 480])?;
            endpoint.register_image(2, [640, 480], &[
                private_images::buffer(device, ExportAccess::ReadWrite)?,
            ])?;
            endpoint.publish(None, |_| Ok(()))?;
            check(endpoint.constraints_id()? != old)
        })
    }
}
