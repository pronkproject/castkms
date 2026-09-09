// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use kernel::dma_fence::{
    testing::ManualFence,
    Status, //
};

#[kunit_tests(rust_castkms_producers)]
mod cases {
    use super::*;

    #[test]
    fn implicit_failure_during_wait_survives_cleanup() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        let producer = kernel::dma_fence::testing::fail_when_waited()?;
        fixture
            .drm
            .add_framebuffer_fence(&fb, 0, &producer, kernel::dma_resv::Usage::Write)?;
        fixture.select(&fb, false, 0)?;
        assert_eq!(producer.status(), Status::Complete(Err(EIO)));
        assert!(fixture.drm.device().output.inspect(|scene| {
            scene.and_then(|scene| scene.producer_status()) == Some(Status::Complete(Err(EIO)))
        }));
        fixture.select(&fb, false, 0)?;
        assert!(fixture
            .drm
            .device()
            .output
            .inspect(|scene| { scene.is_some_and(|scene| scene.producer_status().is_none()) }));
        Ok(())
    }

    #[test]
    fn explicit_sync_excludes_implicit_writers_but_keeps_kernel_work() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        let writer = ManualFence::new()?;
        let kernel = kernel::dma_fence::testing::fail_when_waited()?;
        fixture.drm.add_framebuffer_fence(
            &fb,
            0,
            &writer.fence(),
            kernel::dma_resv::Usage::Write,
        )?;
        fixture
            .drm
            .add_framebuffer_fence(&fb, 0, &kernel, kernel::dma_resv::Usage::Kernel)?;
        let mut explicit = ManualFence::new()?;
        explicit.complete(Ok(()))?;
        fixture.select_with_producer(&fb, false, 0, Some(&explicit.fence()))?;
        assert_eq!(writer.fence().status(), Status::Pending);
        assert_eq!(kernel.status(), Status::Complete(Err(EIO)));
        assert!(fixture
            .drm
            .device()
            .output
            .inspect(|scene| { scene.is_some_and(|scene| scene.producer_failed()) }));
        Ok(())
    }

    #[test]
    fn implicit_sync_does_not_wait_for_readers_or_bookkeeping() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        let reader = ManualFence::new()?;
        let bookkeeping = ManualFence::new()?;
        fixture.drm.add_framebuffer_fence(
            &fb,
            0,
            &reader.fence(),
            kernel::dma_resv::Usage::Read,
        )?;
        fixture.drm.add_framebuffer_fence(
            &fb,
            0,
            &bookkeeping.fence(),
            kernel::dma_resv::Usage::Bookkeep,
        )?;
        fixture.select(&fb, false, 0)?;
        assert_eq!(reader.fence().status(), Status::Pending);
        assert_eq!(bookkeeping.fence().status(), Status::Pending);
        assert!(fixture
            .drm
            .device()
            .output
            .inspect(|scene| { scene.is_some_and(|scene| scene.producer_status().is_none()) }));
        Ok(())
    }

    #[test]
    fn test_only_does_not_acquire_or_wait_for_implicit_work() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        let producer = kernel::dma_fence::testing::fail_when_waited()?;
        fixture
            .drm
            .add_framebuffer_fence(&fb, 0, &producer, kernel::dma_resv::Usage::Write)?;
        fixture.select(&fb, true, 0)?;
        assert_eq!(producer.status(), Status::Pending);
        assert!(fixture.drm.device().output.inspect(|scene| scene.is_none()));
        Ok(())
    }

    #[test]
    fn local_framebuffer_reservation_needs_no_pixel_mapping() -> Result {
        use kernel::drm::gem::BaseObject;

        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        let snapshot = fb
            .object::<gem::Object>()?
            .reservation()
            .snapshot(kernel::dma_resv::Usage::Write)?;
        drop(fb);
        drop(fixture);
        assert!(snapshot.is_empty());
        Ok(())
    }

    #[test]
    fn failed_producer_survives_native_wait_cleanup() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        let mut producer = ManualFence::new()?;
        producer.complete(Err(EIO))?;
        fixture.select_with_producer(&fb, false, 0, Some(&producer.fence()))?;
        drop(producer);
        assert!(fixture.drm.device().output.inspect(|scene| {
            scene.and_then(|scene| scene.producer_status()) == Some(Status::Complete(Err(EIO)))
        }));
        Ok(())
    }

    #[test]
    fn producer_failure_during_native_wait_survives_cleanup() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        let producer = kernel::dma_fence::testing::fail_when_waited()?;
        assert_eq!(producer.status(), Status::Pending);
        fixture.select_with_producer(&fb, false, 0, Some(&producer))?;
        drop(producer);
        assert!(fixture.drm.device().output.inspect(|scene| {
            scene.and_then(|scene| scene.producer_status()) == Some(Status::Complete(Err(EIO)))
        }));
        Ok(())
    }

    #[test]
    fn next_update_does_not_inherit_an_old_producer_failure() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        let mut producer = ManualFence::new()?;
        producer.complete(Err(EIO))?;
        fixture.select_with_producer(&fb, false, 0, Some(&producer.fence()))?;
        fixture.select(&fb, false, 0)?;
        assert!(fixture
            .drm
            .device()
            .output
            .inspect(|scene| { scene.is_some_and(|scene| scene.producer_status().is_none()) }));
        Ok(())
    }

    #[test]
    fn test_only_candidate_does_not_replace_accepted_producer() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        let mut producer = ManualFence::new()?;
        producer.complete(Ok(()))?;
        fixture.select_with_producer(&fb, false, 0, Some(&producer.fence()))?;
        let pending = ManualFence::new()?;
        fixture.select_with_producer(&fb, true, 0, Some(&pending.fence()))?;
        drop(pending);
        assert!(fixture.drm.device().output.inspect(|scene| {
            scene.and_then(|scene| scene.producer_status()) == Some(Status::Complete(Ok(())))
        }));
        Ok(())
    }
}
