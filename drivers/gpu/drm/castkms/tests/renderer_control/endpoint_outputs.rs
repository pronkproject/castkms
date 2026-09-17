// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use crate::renderer::job::Completion;
use kernel::drm::gem::ExportAccess;

#[kunit_tests(rust_castkms_endpoint_outputs)]
mod cases {
    use super::*;

    #[test]
    fn delegated_capture_uses_an_independent_private_to_recipient_claim() -> Result {
        let display = CastKms::new_constraints(c"castkms-endpoint-output", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let endpoint = endpoints::prepared(device, &owner)?;
            endpoint.publish(|_| Ok(()))?;
            let entry = device.constraints_output(crtc)?.lookup(endpoint.constraints_id()?)?;
            device.atomic_update(|state| state.add_crtc_state(crtc)?.set_constraints(&entry))?;

            let source = endpoint.begin_source(1)?;
            let source_id = source.id();
            let serial = source.scene_description()?.content_serial;
            source.publish(|| ())?;
            endpoint.release_source(source_id, Completion::Cpu)?;

            let grant = capture_grant(&file, crtc, connector)?;
            let scope = grant.capture().describe_delegated()?;
            let registration = scope.register_queue(2)?;
            let backing = private_images::buffer(device, ExportAccess::ReadWrite)?;
            let destination = scope.register_destination(
                &backing, drm::fourcc::XRGB8888, drm::fourcc::FORMAT_MOD_LINEAR, 2560, 0,
            )?;
            registration.with_queue(|queue| queue.queue_to(7, &destination, None))?;

            let pending = endpoint.begin_output(1)?;
            check(pending.image_id() == 1)?;
            check(core::ptr::eq(pending.destination().unwrap().buffer(), &*backing))?;
            let output_id = pending.id();
            pending.publish(|| ())?;
            check(endpoint.begin_source(1).err() == Some(ENODATA))?;
            endpoint.release_output(output_id, Completion::Cpu)?;
            endpoint.release_output(output_id, Completion::Cpu)?;

            registration.with_queue(|queue| {
                queue.advance();
                queue.dequeue(|result| {
                    check(result.use_id == 7 && result.result.is_ok())?;
                    check(result.content.is_some_and(|content| content.get() == serial))
                })
            })?;
            Ok(())
        })
    }

    #[test]
    fn failed_output_publication_grants_no_destination_access() -> Result {
        let display = CastKms::new_constraints(c"castkms-endpoint-output-publish", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let endpoint = endpoints::prepared(device, &owner)?;
            endpoint.publish(|_| Ok(()))?;
            let entry = device.constraints_output(crtc)?.lookup(endpoint.constraints_id()?)?;
            device.atomic_update(|state| state.add_crtc_state(crtc)?.set_constraints(&entry))?;
            let source = endpoint.begin_source(1)?;
            let id = source.id();
            source.publish(|| ())?;
            endpoint.release_source(id, Completion::Cpu)?;

            let grant = capture_grant(&file, crtc, connector)?;
            let scope = grant.capture().describe_delegated()?;
            let registration = scope.register_queue(1)?;
            let backing = private_images::buffer(device, ExportAccess::ReadWrite)?;
            let destination = scope.register_destination(
                &backing, drm::fourcc::XRGB8888, drm::fourcc::FORMAT_MOD_LINEAR, 2560, 0,
            )?;
            registration.with_queue(|queue| queue.queue_to(1, &destination, None))?;
            let pending = endpoint.begin_output(1)?;
            drop(pending);
            registration.with_queue(|queue| {
                queue.advance();
                queue.dequeue(|result| check(result.result == Err(ECANCELED)))
            })
        })
    }
}
