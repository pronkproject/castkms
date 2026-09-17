// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use crate::renderer::endpoint::Endpoint;
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
    endpoint.submit_probe(None)?;
    Ok(endpoint)
}

#[kunit_tests(rust_castkms_renderer_endpoints)]
mod cases {
    use super::*;

    #[test]
    fn racing_close_cannot_leave_a_selectable_unowned_offer() -> Result {
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
                let result = endpoint.publish(|_| Ok(()));
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
    fn failed_publication_restores_draft_without_leaking_registration_pins() -> Result {
        let display = CastKms::new_constraints(c"castkms-endpoint-retry", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let endpoint = Endpoint::new(owner.access(), device.to_registered_ref())?;
            check(endpoint.constraints_id()? == 0)?;
            check(endpoint.publish(|_| Ok(())) == Err(ENODATA))?;
            endpoint.declare(private_images::profile()?, [640, 480])?;
            check(endpoint.publish(|_| Ok(())) == Err(ENODATA))?;
            let buffer = private_images::buffer(device, ExportAccess::ReadWrite)?;
            endpoint.register_image(1, [640, 480], &[buffer.clone()])?;
            check(endpoint.publish(|_| Ok(())) == Err(ENODATA))?;
            endpoint.submit_probe(None)?;
            let output = device.constraints_output(crtc)?;
            let generation = output.snapshot(0)?.info().generation;
            check(endpoint.publish(|_| Err(EFAULT)) == Err(EFAULT))?;
            check(output.snapshot(0)?.info().generation == generation)?;
            check(endpoint.constraints_id()? == 0)?;
            endpoint.unregister_image(1)?;
            endpoint.register_image(2, [640, 480], &[buffer.clone()])?;
            let mut id = 0;
            endpoint.publish(|value| { id = value; Ok(()) })?;
            check(id != 0 && endpoint.constraints_id()? == id)?;
            check(endpoint.unregister_image(2) == Err(EBUSY))?;
            check(endpoint.register_image(3, [640, 480], &[buffer]) == Err(EBUSY))?;
            check(endpoint.publish(|_| Ok(())) == Err(EALREADY))?;
            check(endpoint.declare(private_images::profile()?, [640, 480]) == Err(EALREADY))?;
            check(output.lookup(id).is_ok())?;
            drop(endpoint);
            check(output.lookup(id).err() == Some(ESTALE))?;
            check(output.snapshot(0)?.info().selected_id == output.default_entry().id())?;
            Ok(())
        })
    }

    #[test]
    fn failed_probe_never_lists_an_offer_and_close_is_terminal() -> Result {
        let display = CastKms::new_constraints(c"castkms-endpoint-probe", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let endpoint = Endpoint::new(owner.access(), device.to_registered_ref())?;
            endpoint.declare(private_images::profile()?, [640, 480])?;
            endpoint.register_image(1, [640, 480], &[
                private_images::buffer(device, ExportAccess::ReadWrite)?,
            ])?;
            let mut fence = kernel::dma_fence::testing::ManualFence::new()?;
            endpoint.submit_probe(Some(fence.fence()))?;
            check(endpoint.publish(|_| Ok(())) == Err(EAGAIN))?;
            fence.complete(Err(EIO))?;
            check(endpoint.publish(|_| Ok(())) == Err(EIO))?;
            endpoint.unregister_image(1)?;
            check(device.constraints_output(crtc)?.snapshot(0)?.info().count == 1)?;
            endpoint.close();
            check(endpoint.constraints_id() == Err(EKEYREVOKED))?;
            check(endpoint.publish(|_| Ok(())) == Err(EKEYREVOKED))?;
            check(endpoint.declare(private_images::profile()?, [640, 480]) == Err(EKEYREVOKED))?;
            check(endpoint.unregister_image(1) == Err(EKEYREVOKED))?;
            check(endpoint.submit_probe(None) == Err(EKEYREVOKED))?;
            endpoint.close();
            Ok(())
        })
    }

    #[test]
    fn replacement_endpoint_cannot_extend_the_selected_workers_lifetime() -> Result {
        let display = CastKms::new_constraints(c"castkms-endpoint-replace", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let first = prepared(device, &owner)?;
            let second = prepared(device, &owner)?;
            first.publish(|_| Ok(()))?;
            second.publish(|_| Ok(()))?;
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
}
