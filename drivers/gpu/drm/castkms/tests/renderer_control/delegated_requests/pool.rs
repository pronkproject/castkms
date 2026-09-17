// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use crate::renderer::private_pool::Pool;

#[kunit_tests(rust_castkms_private_pool)]
mod cases {
    use super::*;

    #[test]
    fn exact_geometry_pins_preserve_registration_names() -> Result {
        with_output(|fixture| {
            let mut pool = Pool::new()?;
            check(fixture.renderer.pin_private_images(&pool, [640, 480]).err() == Some(ENODATA))?;
            check(fixture.renderer.pin_private_images(&pool, [0, 480]).err() == Some(EINVAL))?;
            pool.insert(1, || Ok(fixture.private.clone()))?;
            check(fixture.renderer.pin_private_images(&pool, [800, 600]).err() == Some(ENODATA))?;
            let first = fixture.renderer.pin_private_images(&pool, [640, 480])?;
            check(first.images().count() == 1)?;
            let (id, image) = first.images().next().ok_or(EINVAL)?;
            check(id == 1 && core::ptr::eq(image, &*fixture.private))?;
            check(pool.remove(1).err() == Some(EBUSY))?;
            let second = fixture.renderer.pin_private_images(&pool, [640, 480])?;
            drop(first);
            check(pool.remove(1).err() == Some(EBUSY))?;
            drop(second);
            drop(pool.remove(1)?);
            check(pool.check(1) == Err(ESTALE))
        })
    }

    #[test]
    fn registration_sets_do_not_adopt_later_names() -> Result {
        with_output(|fixture| {
            let mut pool = Pool::new()?;
            pool.insert(1, || Ok(fixture.private.clone()))?;
            let first = fixture.renderer.pin_private_images(&pool, [640, 480])?;
            pool.insert(2, || Ok(fixture.private.clone()))?;
            check(first.images().count() == 1)?;
            drop(pool.remove(2)?);
            drop(pool);
            // Endpoint removal does not release storage held by the immutable set.
            check(first.images().next().ok_or(EINVAL)?.1.dimensions() == [640, 480])?;
            Ok(())
        })
    }

    #[test]
    fn replacement_cannot_pin_another_candidates_storage() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let fixture = output_fixture(device, crtc, connector, &file)?;
            let replacement = Arc::new(Candidate::begin(fixture.owner.access())?, GFP_KERNEL)?;
            let mut pool = Pool::new()?;
            pool.insert(1, || Ok(fixture.private.clone()))?;
            check(replacement.pin_private_images(&pool, [640, 480]).err() == Some(ENODATA))?;
            let proposal =
                replacement.propose_profile(super::super::super::private_images::profile()?)?;
            let image =
                proposal.register_image([640, 480], &[buffer(device, ExportAccess::ReadWrite)?])?;
            pool.insert(2, || Ok(image.clone()))?;
            let registrations = replacement.pin_private_images(&pool, [640, 480])?;
            check(registrations.images().count() == 1)?;
            check(registrations.images().next().ok_or(EINVAL)?.0 == 2)?;
            drop(pool.remove(1)?);
            check(pool.remove(2).err() == Some(EBUSY))?;
            drop(registrations);
            drop(pool.remove(2)?);
            Ok(())
        })
    }

    #[test]
    fn registration_pins_do_not_complete_native_reads() -> Result {
        with_output(|fixture| {
            let mut pool = Pool::new()?;
            pool.insert(1, || Ok(fixture.private.clone()))?;
            let registrations = fixture.renderer.pin_private_images(&pool, [640, 480])?;
            let request = fixture.destination.request(None)?;
            let claim = request
                .try_claim(&fixture.renderer, &fixture.active, &fixture.rendered)?
                .ok_or(EINVAL)?;
            drop(fixture.rendered);
            let mut completion = ManualFence::new()?;
            claim.release(Completion::Submitted(completion.fence()));
            drop(registrations);
            drop(pool.remove(1)?);
            check(request.status() == Status::Pending)?;
            check(fixture.private.prepare(2).err() == Some(EBUSY))?;
            completion.complete(Ok(()))?;
            wait(&request, Status::Complete(Ok(())))?;
            private_available(&fixture.private, 2)
        })
    }

    #[test]
    fn failed_registration_and_removed_names_preserve_namespace_rules() -> Result {
        with_output(|fixture| {
            let mut pool = Pool::new()?;
            check(pool.check(0) == Err(EINVAL))?;
            check(pool.insert(1, || Err(ENOMEM)) == Err(ENOMEM))?;
            pool.insert(1, || Ok(fixture.private.clone()))?;
            check(Arc::ptr_eq(&pool.image(1)?, &fixture.private))?;
            check(pool.completed(1).err() == Some(ENODATA))?;
            check(pool.insert(1, || Err(EIO)) == Err(ESTALE))?;
            drop(pool.remove(1)?);
            check(pool.image(1).err() == Some(ENOENT))?;
            check(pool.check(1) == Err(ESTALE))?;
            pool.insert(u64::MAX, || Ok(fixture.private.clone()))?;
            drop(pool.remove(u64::MAX)?);
            check(pool.check(u64::MAX) == Err(EOVERFLOW))
        })
    }

    #[test]
    fn withdrawal_does_not_end_outstanding_native_output_access() -> Result {
        with_output(|fixture| {
            let mut pool = Pool::new()?;
            pool.insert(1, || Ok(fixture.private.clone()))?;
            check(pool.publish(1, fixture.rendered.clone())?.is_none())?;
            drop(fixture.rendered);
            let request = fixture.destination.request(None)?;
            let rendered = pool.completed(1)?;
            let claim = request
                .try_claim(&fixture.renderer, &fixture.active, &rendered)?
                .ok_or(EINVAL)?;
            drop(rendered);
            let mut completion = ManualFence::new()?;
            claim.release(Completion::Submitted(completion.fence()));
            drop(pool.withdraw(1)?);
            check(pool.completed(1).err() == Some(ENODATA))?;
            check(fixture.private.prepare(2).err() == Some(EBUSY))?;
            drop(pool.remove(1)?);
            check(request.status() == Status::Pending)?;
            completion.complete(Ok(()))?;
            wait(&request, Status::Complete(Ok(())))?;
            private_available(&fixture.private, 2)
        })
    }

    #[test]
    fn completed_content_must_match_the_exact_registered_image() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let fixture = output_fixture(device, crtc, connector, &file)?;
            let other = fixture.renderer.register_private_image(
                &fixture.active,
                [640, 480],
                &[buffer(device, ExportAccess::ReadWrite)?],
            )?;
            let mut pool = Pool::new()?;
            pool.insert(1, || Ok(other.clone()))?;
            check(pool.publish(1, fixture.rendered).err() == Some(EINVAL))?;
            check(pool.completed(1).err() == Some(ENODATA))?;
            drop(other.prepare(1)?);
            Ok(())
        })
    }

    #[test]
    fn bounded_table_recovers_capacity_without_reusing_names() -> Result {
        with_output(|fixture| {
            let mut pool = Pool::new()?;
            // Namespace accounting is independent of the allocation registry. Each public
            // insertion supplies a separately validated registration through its creator.
            for id in 1..=128 {
                pool.insert(id, || Ok(fixture.private.clone()))?;
            }
            check(pool.check(129) == Err(EBUSY))?;
            drop(pool.remove(1)?);
            pool.insert(129, || Ok(fixture.private.clone()))?;
            check(pool.check(130) == Err(EBUSY))
        })
    }
}
