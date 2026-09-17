// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use crate::renderer::private_pool::Pool;
use kernel::drm::gem::ExportAccess;

#[kunit_tests(rust_castkms_native_constraints)]
mod cases {
    use super::*;

    #[test]
    fn source_reads_and_completed_content_require_the_exact_binding() -> Result {
        use crate::renderer::job::SourceJob;

        let display = CastKms::new_constraints(c"castkms-native-source", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let provider = crtc.display.constraints.as_ref().ok_or(EINVAL)?;
            let control = device.constraints_output(crtc)?;
            let owner = owner(&file, crtc, connector)?;
            let access = owner.access();
            let draft = crate::renderer::draft::Draft::new(
                access.clone(), private_images::profile()?, [640, 480],
            )?;
            let mut pool = Pool::new()?;
            pool.insert(1, || {
                draft.register_image(
                    &[private_images::buffer(device, ExportAccess::ReadWrite)?],
                )
            })?;
            draft.submit_probe(None)?;
            let ready = draft.prepare_worker(&pool)?;
            let other = draft.prepare_worker(&pool)?;
            let worker = ready.worker();
            let first = provider.prepare(worker.clone())?;
            let second = provider.prepare(worker.clone())?;
            provider.publish(&control, &first)?;
            provider.publish(&control, &second)?;
            let claim = |entry: &kernel::drm::constraints::Entry<_>, previous| {
                access.with_current(|current| {
                    let guard = worker.hold_ready()?;
                    SourceJob::claim_bound(&current, entry, &guard, previous)
                })
            };
            check(claim(&first, None).err() == Some(ESTALE))?;
            device.atomic_update(|state| state.add_crtc_state(crtc)?.set_constraints(&first))?;
            check(claim(&second, None).err() == Some(ESTALE))?;
            check(access.with_current(|current| {
                let other = other.worker();
                let guard = other.hold_ready()?;
                SourceJob::claim_bound(&current, &first, &guard, None)
            }).err() == Some(EACCES))?;
            let job = claim(&first, None)?;
            let serial = job.scene().content_serial().map(|serial| serial.get());
            let completed = job.release_cpu();
            access.with_current(|current| completed.check_bound(&current))?;
            check(claim(&first, serial).err() == Some(ENODATA))?;
            device.atomic_update(|state| state.add_crtc_state(crtc)?.set_constraints(&second))?;
            check(access.with_current(|current| completed.check_bound(&current))
                == Err(ESTALE))?;
            check(claim(&first, None).err() == Some(ESTALE))?;
            claim(&second, None)?.release_without_access();
            let image = pool.image(1)?;
            check(crate::renderer::render_job::RenderJob::claim_bound(
                &access, &second, 2, None, image.prepare(1)?,
            ).err() == Some(EACCES))?;
            let job = crate::renderer::render_job::RenderJob::claim_bound(
                &access, &second, 1, None, image.prepare(2)?,
            )?;
            check(core::ptr::eq(job.destination(), &*image))?;
            job.release(crate::renderer::job::Completion::WithoutAccess);
            let mut fence = kernel::dma_fence::testing::ManualFence::new()?;
            let job = claim(&second, None)?;
            let hold = crtc.display.output.with_accepted(|accepted| {
                accepted.ok_or(EINVAL)?.source.hold_admission()
            })?;
            check(hold.prepared()?.is_none())?;
            owner.revoke();
            check(hold.prepared()?.is_none())?;
            check(claim(&second, None).err() == Some(EKEYREVOKED))?;
            let completed = job.release_submitted(fence.fence());
            let prepared = hold.prepared()?.ok_or(EINVAL)?;
            let completion = prepared.completion()?.ok_or(EINVAL)?;
            check(completion.status() == kernel::dma_fence::Status::Pending)?;
            drop(pool.remove(1)?);
            fence.complete(Err(EIO))?;
            check(completion.status() == kernel::dma_fence::Status::Complete(Err(EIO)))?;
            check(completed.status() == kernel::dma_fence::Status::Complete(Err(EIO)))?;
            Ok(())
        })
    }

    #[test]
    fn native_selection_retains_the_exact_ready_worker() -> Result {
        let display = CastKms::new_constraints(c"castkms-native-constraints", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let provider = crtc.display.constraints.as_ref().ok_or(EINVAL)?;
            let control = device.constraints_output(crtc)?;
            check(core::ptr::eq(&*control.selected(), provider.initial()))?;
            let owner = owner(&file, crtc, connector)?;
            let draft = crate::renderer::draft::Draft::new(
                owner.access(), private_images::profile()?, [640, 480],
            )?;
            let mut pool = Pool::new()?;
            pool.insert(1, || {
                draft.register_image(
                    &[private_images::buffer(device, ExportAccess::ReadWrite)?],
                )
            })?;
            draft.submit_probe(None)?;
            let ready = draft.prepare_worker(&pool)?;
            let entry = provider.prepare(ready.worker())?;
            check(entry.description().output().minimum() == (640, 480))?;
            check(entry.description().output().maximum() == (640, 480))?;
            check(control.lookup(entry.id()).err() == Some(ESTALE))?;
            provider.publish(&control, &entry)?;
            check(core::ptr::eq(&*control.selected(), provider.initial()))?;
            device.atomic_update(|state| state.add_crtc_state(crtc)?.set_constraints(&entry))?;
            check(core::ptr::eq(&*control.selected(), &**entry))?;
            let scene = crtc
                .display
                .output
                .with_accepted(|accepted| accepted.and_then(|accepted| accepted.scene.cloned()))
                .ok_or(EINVAL)?;
            check(core::ptr::eq(scene.constraints().ok_or(EINVAL)?, &**entry))?;
            check(!scene.host_binding())?;
            let host_pool = crate::host_compositor::pool::Pool::new(
                device,
                &crate::host_compositor::budget::Budget::new()?,
                crate::host_compositor::layout::Layout::new(640, 480)?,
            )?;
            check(
                crate::host_compositor::compose::current(&crtc.display.output, &host_pool).err()
                    == Some(EOPNOTSUPP),
            )?;
            owner.revoke();
            check(ready.worker().hold_ready().err() == Some(EKEYREVOKED))?;
            check(
                device.atomic_update(|state| {
                    state.add_crtc_state(crtc)?;
                    Ok(())
                }) == Err(EKEYREVOKED),
            )?;
            device.atomic_update(|state| state.set_crtc_config(crtc, None))?;
            check(core::ptr::eq(&*control.selected(), &**entry))?;
            control.restore_default()?;
            check(core::ptr::eq(&*control.selected(), provider.initial()))?;
            // A retained old scene remains attributed to the old worker, never HOST.
            check(!scene.host_binding())?;
            control.withdraw(entry.id())?;
            control.forget(entry.id())?;
            drop(provider.remove(&entry));
            drop(pool.remove(1)?);
            Ok(())
        })
    }

    #[test]
    fn mixed_format_offer_does_not_apply_yuv_values_to_rgb_scanout() -> Result {
        use crate::execution::capabilities::{Format, Profile};

        let display = CastKms::new_constraints(c"castkms-native-mixed-color", 1)?;
        with_registered_display(&display, |device, crtc, connector, scanout, file| {
            let provider = crtc.display.constraints.as_ref().ok_or(EINVAL)?;
            let control = device.constraints_output(crtc)?;
            let owner = owner(&file, crtc, connector)?;
            let mut limits = *private_images::profile()?.limits();
            limits.color.yuv_encodings = [false, true, false];
            limits.color.yuv_ranges = [false, true];
            let mut formats = KVec::new();
            for (fourcc, planes) in [(drm::fourcc::XRGB8888, 1), (drm::fourcc::NV12, 2)] {
                formats.push(
                    Format {
                        fourcc,
                        modifier: None,
                        planes,
                        native: true,
                        imported: true,
                        pitch_alignment: 1,
                        offset_alignment: 1,
                        max_pitch: u32::MAX,
                    },
                    GFP_KERNEL,
                )?;
            }
            let draft = crate::renderer::draft::Draft::new(
                owner.access(), Profile::new(limits, formats)?, [640, 480],
            )?;
            let mut pool = Pool::new()?;
            pool.insert(1, || {
                draft.register_image(
                    &[private_images::buffer(device, ExportAccess::ReadWrite)?],
                )
            })?;
            draft.submit_probe(None)?;
            let ready = draft.prepare_worker(&pool)?;
            let entry = provider.prepare(ready.worker())?;
            provider.publish(&control, &entry)?;

            device.atomic_update(|mut state| {
                state.as_mut().set_crtc_config(crtc, Some(scanout))?;
                state.add_crtc_state(crtc)?.set_constraints(&entry)
            })?;
            check(core::ptr::eq(&*control.selected(), &**entry))?;

            device.atomic_update(|state| state.set_crtc_config(crtc, None))?;
            control.restore_default()?;
            control.withdraw(entry.id())?;
            control.forget(entry.id())?;
            drop(provider.remove(&entry));
            drop(ready);
            drop(pool.remove(1)?);
            Ok(())
        })
    }

    #[test]
    fn exact_tiled_worker_admits_tiled_scanout() -> Result {
        let display = CastKms::new_constraints(c"castkms-native-tiled", 1)?;
        with_registered_display(&display, |device, crtc, connector, scanout, file| {
            let provider = crtc.display.constraints.as_ref().ok_or(EINVAL)?;
            let control = device.constraints_output(crtc)?;
            let owner = owner(&file, crtc, connector)?;
            let modifier = drm::fourcc::I915_FORMAT_MOD_4_TILED;
            let draft = crate::renderer::draft::Draft::new(
                owner.access(), private_images::profile_for(Some(modifier))?, [640, 480],
            )?;
            let mut pool = Pool::new()?;
            pool.insert(1, || {
                draft.register_image(
                    &[private_images::buffer(device, ExportAccess::ReadWrite)?],
                )
            })?;
            draft.submit_probe(None)?;
            let ready = draft.prepare_worker(&pool)?;
            let entry = provider.prepare(ready.worker())?;
            provider.publish(&control, &entry)?;
            let object = shmem::Object::<gem::Object>::new(
                device, 640 * 480 * 4, Default::default(), Default::default(),
            )?;
            let framebuffer = Framebuffer::from_objects(device, &FramebufferLayout {
                width: 640, height: 480, format: drm::fourcc::XRGB8888,
                modifier: Some(modifier), interlaced: false,
                planes: &[FramebufferPlane { object: &object, pitch: 2560, offset: 0 }],
            })?;
            let tiled = CrtcScanout {
                mode: scanout.mode,
                framebuffer: &framebuffer,
                connectors: scanout.connectors,
                position: scanout.position,
            };

            check(device.atomic_update(|state| {
                state.set_crtc_config(crtc, Some(&tiled))
            }) == Err(EINVAL))?;
            device.atomic_update(|mut state| {
                state.as_mut().set_crtc_config(crtc, Some(&tiled))?;
                state.add_crtc_state(crtc)?.set_constraints(&entry)
            })?;
            let scene = crtc.display.output.with_accepted(|accepted| {
                accepted.and_then(|accepted| accepted.scene.cloned())
            }).ok_or(EINVAL)?;
            check(scene.primary().ok_or(EINVAL)?.framebuffer().modifier() == Some(modifier))?;
            check(!scene.host_binding())?;
            device.atomic_update(|state| state.set_crtc_config(crtc, None))?;
            control.restore_default()?;
            control.withdraw(entry.id())?;
            control.forget(entry.id())?;
            drop(provider.remove(&entry));
            drop(ready);
            drop(pool.remove(1)?);
            Ok(())
        })
    }

    #[test]
    fn cancelled_worker_is_never_published() -> Result {
        let display = CastKms::new_constraints(c"castkms-native-cancel", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let provider = crtc.display.constraints.as_ref().ok_or(EINVAL)?;
            let control = device.constraints_output(crtc)?;
            let owner = owner(&file, crtc, connector)?;
            let draft = crate::renderer::draft::Draft::new(
                owner.access(), private_images::profile()?, [640, 480],
            )?;
            let mut pool = Pool::new()?;
            pool.insert(1, || {
                draft.register_image(
                    &[private_images::buffer(device, ExportAccess::ReadWrite)?],
                )
            })?;
            draft.submit_probe(None)?;
            let ready = draft.prepare_worker(&pool)?;
            let entry = provider.prepare(ready.worker())?;
            drop(ready);
            check(provider.publish(&control, &entry) == Err(EKEYREVOKED))?;
            check(control.lookup(entry.id()).err() == Some(ESTALE))?;
            check(provider.resolve(&entry).err() == Some(ESTALE))?;
            drop(pool.remove(1)?);
            Ok(())
        })
    }

    #[test]
    fn failed_native_publication_removes_only_the_new_index_reference() -> Result {
        let display = CastKms::new_constraints(c"castkms-native-rollback", 1)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let provider = crtc.display.constraints.as_ref().ok_or(EINVAL)?;
            let control = device.constraints_output(crtc)?;
            let owner = owner(&file, crtc, connector)?;
            let draft = crate::renderer::draft::Draft::new(
                owner.access(), private_images::profile()?, [640, 480],
            )?;
            let mut pool = Pool::new()?;
            pool.insert(1, || {
                draft.register_image(
                    &[private_images::buffer(device, ExportAccess::ReadWrite)?],
                )
            })?;
            draft.submit_probe(None)?;
            let ready = draft.prepare_worker(&pool)?;
            let entry = provider.prepare(ready.worker())?;
            // Inject native membership without provider membership to force add failure.
            control.add(&entry)?;
            check(provider.publish(&control, &entry) == Err(EEXIST))?;
            check(provider.resolve(&entry).err() == Some(ESTALE))?;
            check(core::ptr::eq(&*control.lookup(entry.id())?, &**entry))?;
            control.withdraw(entry.id())?;
            control.forget(entry.id())?;
            let next = provider.prepare(ready.worker())?;
            provider.publish(&control, &next)?;
            check(provider.publish(&control, &next) == Err(EEXIST))?;
            check(core::ptr::eq(&*provider.resolve(&next)?, &**next))?;
            let pending = provider.prepare(ready.worker())?;
            provider.close();
            check(provider.publish(&control, &pending) == Err(ESHUTDOWN))?;
            check(control.lookup(pending.id()).err() == Some(ESTALE))?;
            drop(pool.remove(1)?);
            Ok(())
        })
    }

    #[test]
    fn wrong_output_cannot_adopt_a_ready_worker() -> Result {
        let display = CastKms::new_constraints(c"castkms-native-scope", 2)?;
        with_registered_display(&display, |device, crtc, connector, _, file| {
            let provider = crtc.display.constraints.as_ref().ok_or(EINVAL)?;
            let other = device
                .displays
                .iter()
                .find(|other| other.output.identity() != crtc.display.output.identity())
                .ok_or(EINVAL)?;
            let other = other.constraints.as_ref().ok_or(EINVAL)?;
            let owner = owner(&file, crtc, connector)?;
            let draft = crate::renderer::draft::Draft::new(
                owner.access(), private_images::profile()?, [640, 480],
            )?;
            let mut pool = Pool::new()?;
            pool.insert(1, || {
                draft.register_image(
                    &[private_images::buffer(device, ExportAccess::ReadWrite)?],
                )
            })?;
            draft.submit_probe(None)?;
            let ready = draft.prepare_worker(&pool)?;
            check(other.prepare(ready.worker()).err() == Some(EINVAL))?;
            let entry = provider.prepare(ready.worker())?;
            provider.publish(&device.constraints_output(crtc)?, &entry)?;
            check(other.resolve(&entry).err() == Some(ESTALE))?;
            provider.close();
            check(ready.worker().hold_ready().err() == Some(EKEYREVOKED))?;
            check(provider.resolve(&entry).err() == Some(ESHUTDOWN))?;
            check(provider.prepare(ready.worker()).err() == Some(ESHUTDOWN))?;
            drop(pool.remove(1)?);
            Ok(())
        })
    }
}
