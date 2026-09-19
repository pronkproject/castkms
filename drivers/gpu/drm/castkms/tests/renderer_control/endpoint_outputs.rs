// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use crate::renderer::job::Completion;
use kernel::drm::gem::ExportAccess;

#[kunit_tests(rust_castkms_endpoint_outputs)]
mod cases {
    use super::*;
    use crate::renderer::endpoint::Endpoint;

    #[test]
    fn output_poll_is_idle_before_publication() -> Result {
        let display = CastKms::new_constraints(c"castkms-endpoint-output-idle", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let endpoint = Endpoint::new(owner.access(), device.to_registered_ref())?;
            check(!endpoint.output_readable()?)?;
            let endpoint = endpoints::prepared(device, &owner)?;
            check(!endpoint.output_readable()?)
        })
    }

    #[test]
    fn delegated_capture_uses_an_independent_private_to_recipient_claim() -> Result {
        let display = CastKms::new_constraints(c"castkms-endpoint-output", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let endpoint = endpoints::prepared(device, &owner)?;
            endpoint.publish(None, |_| Ok(()))?;
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
    fn output_poll_skips_stale_completed_images() -> Result {
        let display = CastKms::new_constraints(c"castkms-endpoint-output-interval", 1)?;
        with_registered_display(&display, |device, crtc, connector, scanout, file| {
            let owner = owner(&file, crtc, connector)?;
            let endpoint = endpoints::prepared(device, &owner)?;
            endpoint.register_image(2, [640, 480], &[
                private_images::buffer(device, ExportAccess::ReadWrite)?,
            ])?;
            endpoint.publish(None, |_| Ok(()))?;
            let entry = device.constraints_output(crtc)?.lookup(endpoint.constraints_id()?)?;
            device.atomic_update(|state| state.add_crtc_state(crtc)?.set_constraints(&entry))?;

            let first = endpoint.begin_source(1)?;
            let first_id = first.id();
            first.publish(|| ())?;
            endpoint.release_source(first_id, Completion::Cpu)?;

            device.atomic_update(|state| state.set_crtc_config(crtc, None))?;
            device.atomic_update(|mut state| {
                state.as_mut().set_crtc_config(crtc, Some(scanout))?;
                state.add_crtc_state(crtc)?.set_constraints(&entry)
            })?;
            let second = endpoint.begin_source(2)?;
            let second_id = second.id();
            second.publish(|| ())?;
            endpoint.release_source(second_id, Completion::Cpu)?;
            check(endpoint.completed_image(1).err() == Some(ESTALE))?;
            check(endpoint.completed_image(2).is_ok())?;

            let grant = capture_grant(&file, crtc, connector)?;
            let scope = grant.capture().describe_delegated()?;
            let registration = scope.register_queue(1)?;
            let backing = private_images::buffer(device, ExportAccess::ReadWrite)?;
            let destination = scope.register_destination(
                &backing, drm::fourcc::XRGB8888, drm::fourcc::FORMAT_MOD_LINEAR, 2560, 0,
            )?;
            registration.with_queue(|queue| queue.queue_to(7, &destination, None))?;
            check(endpoint.output_readable()?)?;
            let output = endpoint.begin_output(2)?;
            let output_id = output.id();
            output.publish(|| ())?;
            endpoint.release_output(output_id, Completion::Cpu)?;
            registration.with_queue(|queue| {
                queue.advance();
                queue.dequeue(|result| check(result.use_id == 7 && result.result.is_ok()))
            })
        })
    }

    #[test]
    fn failed_output_publication_grants_no_destination_access() -> Result {
        let display = CastKms::new_constraints(c"castkms-endpoint-output-publish", 1)?;
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

    #[test]
    fn withdrawn_output_publication_grants_no_destination_access() -> Result {
        let display = CastKms::new_constraints(c"castkms-endpoint-output-withdraw", 1)?;
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

            let grant = capture_grant(&file, crtc, connector)?;
            let scope = grant.capture().describe_delegated()?;
            let registration = scope.register_queue(1)?;
            let backing = private_images::buffer(device, ExportAccess::ReadWrite)?;
            let destination = scope.register_destination(
                &backing, drm::fourcc::XRGB8888, drm::fourcc::FORMAT_MOD_LINEAR, 2560, 0,
            )?;
            registration.with_queue(|queue| queue.queue_to(1, &destination, None))?;
            let pending = endpoint.begin_output(1)?;
            endpoint.withdraw()?;
            check(pending.publish(|| ()).err() == Some(EKEYREVOKED))?;
            registration.with_queue(|queue| {
                queue.advance();
                queue.dequeue(|result| check(result.result == Err(ECANCELED)))
            })
        })
    }

    #[test]
    fn revoked_recipient_grant_blocks_output_publication() -> Result {
        let display = CastKms::new_constraints(c"castkms-endpoint-output-revoke", 1)?;
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

            let grant = capture_grant(&file, crtc, connector)?;
            let scope = grant.capture().describe_delegated()?;
            let registration = scope.register_queue(1)?;
            let backing = private_images::buffer(device, ExportAccess::ReadWrite)?;
            let destination = scope.register_destination(
                &backing, drm::fourcc::XRGB8888, drm::fourcc::FORMAT_MOD_LINEAR, 2560, 0,
            )?;
            registration.with_queue(|queue| queue.queue_to(1, &destination, None))?;
            let pending = endpoint.begin_output(1)?;
            drop(grant);
            check(pending.publish(|| ()).err() == Some(EKEYREVOKED))?;
            registration.with_queue(|queue| {
                queue.advance();
                queue.dequeue(|result| check(result.result == Err(EKEYREVOKED)))
            })
        })
    }
}
