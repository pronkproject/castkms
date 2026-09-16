// SPDX-License-Identifier: GPL-2.0-only

use super::{
    delegated_authority::grant,
    private_images::{activate, buffer},
    *,
};
use kernel::{
    dma_fence::testing::ManualFence,
    drm::{
        fourcc,
        gem::{BaseObject, ExportAccess},
    },
    sync::aref::ARef,
};

#[kunit_tests(rust_castkms_delegated_destinations)]
mod cases {
    use super::*;

    #[test]
    fn private_storage_cannot_be_registered_as_a_recipient_destination() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let (active, _) = activate(&candidate, device, crtc)?;
            let grantor = grant(&file, crtc, connector)?;
            let scope = grantor.capture().describe_delegated()?;
            let backing = buffer(device, ExportAccess::ReadWrite)?;
            let private =
                candidate.register_private_image(&active, [640, 480], &[backing.clone()])?;
            check(
                scope
                    .register_destination(&backing, fourcc::XRGB8888, 0, 2560, 0)
                    .err()
                    == Some(EEXIST),
            )?;
            drop(private);
            // No recipient has received this backing; dropping an unused private registration
            // releases accounting, not any external authority over an exported allocation.
            let destination = scope.register_destination(&backing, fourcc::XRGB8888, 0, 2560, 0)?;
            check(
                candidate
                    .register_private_image(&active, [640, 480], &[backing])
                    .err()
                    == Some(EEXIST),
            )?;
            check(destination.layout().dimensions == [640, 480])?;
            check(destination.layout().pitch == 2560 && destination.layout().offset == 0)
        })
    }

    #[test]
    fn another_grant_cannot_register_an_outstanding_destination_alias() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let (_active, _) = activate(&candidate, device, crtc)?;
            let grantor = grant(&file, crtc, connector)?;
            let other = grant(&file, crtc, connector)?;
            let backing = buffer(device, ExportAccess::ReadWrite)?;
            let image = grantor
                .capture()
                .describe_delegated()?
                .register_destination(&backing, fourcc::XRGB8888, 0, 2560, 0)?;
            let usage = image.reserve(1, None)?;
            drop(image);
            drop(grantor);
            check(
                other
                    .capture()
                    .describe_delegated()?
                    .register_destination(&backing, fourcc::XRGB8888, 0, 2560, 0)
                    .err()
                    == Some(EEXIST),
            )?;
            drop(usage);
            Ok(())
        })
    }

    #[test]
    fn destination_reuse_is_typed_without_claiming_a_source() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let (_active, _) = activate(&candidate, device, crtc)?;
            let grantor = grant(&file, crtc, connector)?;
            let image = grantor
                .capture()
                .describe_delegated()?
                .register_destination(
                    &buffer(device, ExportAccess::ReadWrite)?,
                    fourcc::XRGB8888,
                    0,
                    2560,
                    0,
                )?;
            let mut reuse = ManualFence::new()?;
            let usage = image.reserve(1, Some(reuse.fence()))?;
            check(usage.ready() == Ok(false))?;
            check(image.reserve(2, None).err() == Some(EBUSY))?;
            let source = device
                .output
                .with_accepted(|a| a.map(|a| ARef::from(a.source)))
                .ok_or(EINVAL)?;
            source.seal();
            check(source.prepared()?.is_some())?;
            reuse.complete(Err(EAGAIN))?;
            check(usage.ready() == Err(EAGAIN))?;
            drop(usage);
            check(image.reserve(1, None).err() == Some(ESTALE))?;
            drop(image.reserve(2, None)?);
            Ok(())
        })
    }

    #[test]
    fn revoked_destination_rejects_new_uses() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let (_active, _) = activate(&candidate, device, crtc)?;
            let grantor = grant(&file, crtc, connector)?;
            let image = grantor
                .capture()
                .describe_delegated()?
                .register_destination(
                    &buffer(device, ExportAccess::ReadWrite)?,
                    fourcc::XRGB8888,
                    0,
                    2560,
                    0,
                )?;
            drop(grantor);
            check(image.reserve(1, None).err() == Some(EKEYREVOKED))
        })
    }

    #[test]
    fn destination_rejects_source_aliases_and_malformed_rows() -> Result {
        with_display(|device, crtc, connector, scanout, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let (_active, _) = activate(&candidate, device, crtc)?;
            let grantor = grant(&file, crtc, connector)?;
            let scope = grantor.capture().describe_delegated()?;
            let source = scanout
                .framebuffer
                .object_at(0)?
                .export_dma_buf(ExportAccess::ReadWrite)?;
            check(
                scope
                    .register_destination(&source, fourcc::XRGB8888, 0, 2560, 0)
                    .err()
                    == Some(EINVAL),
            )?;
            let backing = buffer(device, ExportAccess::ReadWrite)?;
            for (format, modifier, pitch, offset, error) in [
                (fourcc::ARGB8888, 0, 2560, 0, EOPNOTSUPP),
                (fourcc::XRGB8888, 1, 2560, 0, EOPNOTSUPP),
                (fourcc::XRGB8888, 0, 2556, 0, EINVAL),
                (fourcc::XRGB8888, 0, 2561, 0, EINVAL),
                (fourcc::XRGB8888, 0, 2560, 4, EINVAL),
                (fourcc::XRGB8888, 0, usize::MAX - 3, 0, EOVERFLOW),
            ] {
                check(
                    scope
                        .register_destination(&backing, format, modifier, pitch, offset)
                        .err()
                        == Some(error),
                )?;
            }
            let readonly = buffer(device, ExportAccess::ReadOnly)?;
            check(
                scope
                    .register_destination(&readonly, fourcc::XRGB8888, 0, 2560, 0)
                    .err()
                    == Some(EACCES),
            )
        })
    }

    #[test]
    fn destination_use_ids_do_not_wrap() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let (_active, _) = activate(&candidate, device, crtc)?;
            let grantor = grant(&file, crtc, connector)?;
            let image = grantor
                .capture()
                .describe_delegated()?
                .register_destination(
                    &buffer(device, ExportAccess::ReadWrite)?,
                    fourcc::XRGB8888,
                    0,
                    2560,
                    0,
                )?;
            check(image.reserve(0, None).err() == Some(EINVAL))?;
            drop(image.reserve(u64::MAX, None)?);
            check(image.reserve(u64::MAX, None).err() == Some(EOVERFLOW))
        })
    }

    #[test]
    fn retained_implicit_reuse_failure_survives_reservation_cleanup() -> Result {
        crate::tests::with_exporter(|exporter| {
            let framebuffer =
                exporter.framebuffer(crate::provenance::Provenance::from_snapshot(None))?;
            let backing = framebuffer
                .object_at(0)?
                .export_dma_buf(ExportAccess::ReadWrite)?;
            let mut reuse = ManualFence::new()?;
            exporter.drm.add_framebuffer_fence(
                &framebuffer,
                0,
                &reuse.fence(),
                kernel::dma_resv::Usage::Read,
            )?;
            with_display(|device, crtc, connector, _, file| {
                let owner = owner(&file, crtc, connector)?;
                let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
                let (_active, _) = activate(&candidate, device, crtc)?;
                let grantor = grant(&file, crtc, connector)?;
                let image = grantor
                    .capture()
                    .describe_delegated()?
                    .register_destination(&backing, fourcc::XRGB8888, 0, 2560, 0)?;
                let usage = image.reserve(1, None)?;
                check(usage.ready() == Ok(false))?;
                reuse.complete(Err(EAGAIN))?;
                check(
                    backing
                        .reservation()
                        .snapshot(kernel::dma_resv::Usage::Read)?
                        .is_empty(),
                )?;
                check(usage.ready() == Err(EAGAIN))
            })
        })
    }

    #[test]
    fn newly_observed_reuse_failure_stays_terminal() -> Result {
        crate::tests::with_exporter(|exporter| {
            let framebuffer =
                exporter.framebuffer(crate::provenance::Provenance::from_snapshot(None))?;
            let backing = framebuffer
                .object_at(0)?
                .export_dma_buf(ExportAccess::ReadWrite)?;
            with_display(|device, crtc, connector, _, file| {
                let owner = owner(&file, crtc, connector)?;
                let candidate = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
                let (_active, _) = activate(&candidate, device, crtc)?;
                let grantor = grant(&file, crtc, connector)?;
                let image = grantor
                    .capture()
                    .describe_delegated()?
                    .register_destination(&backing, fourcc::XRGB8888, 0, 2560, 0)?;
                let usage = image.reserve(1, None)?;
                check(usage.ready() == Ok(true))?;
                let mut reuse = ManualFence::new()?;
                exporter.drm.add_framebuffer_fence(
                    &framebuffer,
                    0,
                    &reuse.fence(),
                    kernel::dma_resv::Usage::Read,
                )?;
                check(usage.ready() == Ok(false))?;
                reuse.complete(Err(EIO))?;
                check(
                    backing
                        .reservation()
                        .snapshot(kernel::dma_resv::Usage::Read)?
                        .is_empty(),
                )?;
                check(usage.ready() == Err(EIO))
            })
        })
    }

    #[test]
    fn recipient_registration_capacity_does_not_consume_private_credits() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let renderer = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let (active, _) = activate(&renderer, device, crtc)?;
            let grantor = grant(&file, crtc, connector)?;
            let scope = grantor.capture().describe_delegated()?;
            let mut images = KVec::new();
            for _ in 0..128 {
                images.push(
                    scope.register_destination(
                        &buffer(device, ExportAccess::ReadWrite)?,
                        fourcc::XRGB8888,
                        0,
                        2560,
                        0,
                    )?,
                    GFP_KERNEL,
                )?;
            }
            let next = buffer(device, ExportAccess::ReadWrite)?;
            check(
                scope
                    .register_destination(&next, fourcc::XRGB8888, 0, 2560, 0)
                    .err()
                    == Some(EBUSY),
            )?;
            let private = renderer.register_private_image(
                &active,
                [640, 480],
                &[buffer(device, ExportAccess::ReadWrite)?],
            )?;
            drop(private.prepare(1)?);
            drop(images.pop());
            drop(scope.register_destination(&next, fourcc::XRGB8888, 0, 2560, 0)?);
            Ok(())
        })
    }

    #[test]
    fn recipient_byte_budget_does_not_consume_private_credits() -> Result {
        with_display(|device, crtc, connector, _, file| {
            let owner = owner(&file, crtc, connector)?;
            let renderer = Arc::new(Candidate::begin(owner.access())?, GFP_KERNEL)?;
            let (active, _) = activate(&renderer, device, crtc)?;
            let grantor = grant(&file, crtc, connector)?;
            let scope = grantor.capture().describe_delegated()?;
            let allocate = |size| {
                shmem::Object::<gem::Object>::new(
                    device,
                    size,
                    Default::default(),
                    Default::default(),
                )?
                .export_dma_buf(ExportAccess::ReadWrite)
            };
            let image = scope.register_destination(
                &allocate(400 * 1024 * 1024)?,
                fourcc::XRGB8888,
                0,
                2560,
                0,
            )?;
            let next = allocate(200 * 1024 * 1024)?;
            check(
                scope
                    .register_destination(&next, fourcc::XRGB8888, 0, 2560, 0)
                    .err()
                    == Some(EBUSY),
            )?;
            let private = renderer.register_private_image(
                &active,
                [640, 480],
                &[allocate(400 * 1024 * 1024)?],
            )?;
            drop(private.prepare(1)?);
            drop(image);
            drop(scope.register_destination(&next, fourcc::XRGB8888, 0, 2560, 0)?);
            Ok(())
        })
    }
}
