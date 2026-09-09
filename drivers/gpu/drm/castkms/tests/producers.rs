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
