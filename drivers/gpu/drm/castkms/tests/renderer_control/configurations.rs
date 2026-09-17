// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use crate::renderer::{configuration::Configuration, private_pool::Pool};
use kernel::drm::gem::{BaseObject, ExportAccess};

#[kunit_tests(rust_castkms_configurations)]
mod cases {
    use super::*;

    #[test]
    fn disabled_output_accepts_independent_ready_configurations() -> Result {
        let display = CastKms::new_constraints(c"castkms-publication-configurations", 1)?;
        let device = display._display.registration_guard().ok_or(ENODEV)?;
        let file = RegisteredMasterFile::new(&device)?;
        let crtc = file.crtc()?.to_owned_ref();
        let crtc = crtc.crtc();
        let connector = file.connector()?;
        let owner = owner(&file, crtc, &connector)?;
        let first = Configuration::new(owner.access(), private_images::profile()?, [640, 480])?;
        let second = Configuration::new(owner.access(), private_images::profile()?, [800, 600])?;
        check(first.dimensions() == [640, 480] && second.dimensions() == [800, 600])?;
        check(first.access().with_current(|_| Ok(())) == Err(ENODEV))?;
        let mut first_pool = Pool::new()?;
        let mut second_pool = Pool::new()?;
        first_pool.insert(1, || first.register_image(&[
            private_images::buffer(&device, ExportAccess::ReadWrite)?,
        ]))?;
        let buffer = shmem::Object::<gem::Object>::new(
            &device, (800usize * 600 * 4).next_multiple_of(kernel::page::PAGE_SIZE),
            Default::default(), Default::default(),
        )?;
        second_pool.insert(1, || second.register_image(&[
            buffer.export_dma_buf(ExportAccess::ReadWrite)?,
        ]))?;
        check(first.prepare_worker(&second_pool, None).err() == Some(ENODATA))?;
        let first_ready = first.prepare_worker(&first_pool, None)?;
        let second_ready = second.prepare_worker(&second_pool, None)?;
        let provider = crtc.display.constraints.as_ref().ok_or(EINVAL)?;
        let control = device.constraints_output(crtc)?;
        let first_entry = provider.prepare(first_ready.worker())?;
        let second_entry = provider.prepare(second_ready.worker())?;
        owner.access().with_output(|| {
            provider.publish(&control, &first_entry)?;
            provider.publish(&control, &second_entry)
        })?;
        check(first_entry.description().output().minimum() == (640, 480))?;
        check(second_entry.description().output().minimum() == (800, 600))?;
        check(core::ptr::eq(&*control.selected(), provider.initial()))?;
        device.atomic_update(|state| state.add_crtc_state(crtc)?.set_constraints(&second_entry))?;
        check(core::ptr::eq(&*control.selected(), &**second_entry))?;
        drop(first_ready);
        drop(first_pool.remove(1)?);
        check(second_pool.remove(1).err() == Some(EBUSY))?;
        drop(second_ready.worker().hold_ready()?);
        owner.revoke();
        check(second_ready.worker().hold_ready().err() == Some(EKEYREVOKED))?;
        drop(second_pool.remove(1)?);
        Ok(())
    }

    #[test]
    fn pending_and_failed_readiness_leave_pool_names_unpinned() -> Result {
        let display = CastKms::new_constraints(c"castkms-configuration-readiness", 1)?;
        let device = display._display.registration_guard().ok_or(ENODEV)?;
        let file = RegisteredMasterFile::new(&device)?;
        let crtc = file.crtc()?;
        let connector = file.connector()?;
        let owner = owner(&file, crtc, &connector)?;
        let configuration = Configuration::new(
            owner.access(), private_images::profile()?, [640, 480],
        )?;
        let mut pool = Pool::new()?;
        let image = configuration.register_image(&[
            private_images::buffer(&device, ExportAccess::ReadWrite)?,
        ])?;
        pool.insert(1, || Ok(image.clone()))?;
        let mut completion = kernel::dma_fence::testing::ManualFence::new()?;
        let fence = completion.fence();
        check(configuration.prepare_worker(&pool, Some(&fence)).err() == Some(EBUSY))?;
        drop(pool.remove(1)?);
        pool.insert(2, || Ok(image))?;
        completion.complete(Err(EIO))?;
        check(configuration.prepare_worker(&pool, Some(&fence)).err() == Some(EREMOTEIO))?;
        drop(pool.remove(2)?);
        Ok(())
    }

    #[test]
    fn preparation_checks_geometry_source_aliases_and_revocation() -> Result {
        let display = CastKms::new_constraints(c"castkms-configuration-authority", 1)?;
        with_registered_display(&display, |device, crtc, connector, scanout, file| {
            let owner = owner(&file, crtc, connector)?;
            // Exercise the source-admission lock order alongside disabled configuration checks.
            owner.access().with_current(|_| Ok(()))?;
            check(Configuration::new(owner.access(), private_images::profile()?, [0, 480]).err()
                == Some(EOPNOTSUPP))?;
            let configuration = Configuration::new(
                owner.access(), private_images::profile()?, [800, 600],
            )?;
            let source = scanout.framebuffer.object_at(0)?
                .export_dma_buf(ExportAccess::ReadWrite)?;
            check(configuration.register_image(&[source]).err() == Some(EINVAL))?;
            let storage = shmem::Object::<gem::Object>::new(
                device, (800usize * 600 * 4).next_multiple_of(kernel::page::PAGE_SIZE),
                Default::default(), Default::default(),
            )?.export_dma_buf(ExportAccess::ReadWrite)?;
            let mut pool = Pool::new()?;
            pool.insert(1, || configuration.register_image(&[storage.clone()]))?;
            let ready = configuration.prepare_worker(&pool, None)?;
            check(ready.worker().profile().limits().geometry.min_output == [800, 600])?;
            check(ready.worker().profile().limits().geometry.output == [800, 600])?;
            owner.revoke();
            check(ready.worker().hold_ready().err() == Some(EKEYREVOKED))?;
            check(configuration.prepare_worker(&pool, None).err() == Some(EKEYREVOKED))?;
            check(configuration.register_image(&[storage]).err() == Some(EKEYREVOKED))?;
            drop(pool.remove(1)?);
            Ok(())
        })
    }

    #[test]
    fn preparation_rejects_unusable_allocation_tuples() -> Result {
        let display = CastKms::new_constraints(c"castkms-configuration-allocations", 1)?;
        let device = display._display.registration_guard().ok_or(ENODEV)?;
        let file = RegisteredMasterFile::new(&device)?;
        let crtc = file.crtc()?;
        let connector = file.connector()?;
        let owner = owner(&file, crtc, &connector)?;
        let valid = private_images::profile()?;
        let mut formats = KVec::new();
        formats.push(
            crate::execution::capabilities::Format {
                planes: 2,
                ..valid.formats()[0]
            },
            GFP_KERNEL,
        )?;
        let unusable = crate::execution::capabilities::Profile::new(*valid.limits(), formats)?;

        let endpoint = crate::renderer::endpoint::Endpoint::new(
            owner.access(),
            device.to_registered_ref(),
        )?;
        check(endpoint.declare(unusable, [640, 480]) == Err(EOPNOTSUPP))?;
        endpoint.declare(private_images::profile()?, [640, 480])?;
        Ok(())
    }
}
