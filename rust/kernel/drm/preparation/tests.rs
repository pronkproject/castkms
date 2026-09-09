// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;
use crate::dma_fence::{testing::ManualFence, Status};

#[kunit_tests(rust_drm_preparation)]
mod cases {
    use super::*;

    #[test]
    fn prepared_owner_retains_native_read_completion() -> Result {
        let source = Source::new(1)?;
        let read = source.claim()?;
        let mut native = ManualFence::new()?;
        source.seal();
        assert!(source.prepared()?.is_none());
        read.release_submitted(&native.fence());
        let prepared = source.prepared()?.ok_or(EINVAL)?;
        drop(source);
        let completion = prepared.completion()?.ok_or(EINVAL)?;
        drop(prepared);
        assert_eq!(completion.status(), Status::Pending);
        native.complete(Ok(()))?;
        assert_eq!(completion.status(), Status::Complete(Ok(())));
        Ok(())
    }

    #[test]
    fn dropped_claim_fails_preparation_instead_of_releasing_access() -> Result {
        let source = Source::new(1)?;
        let read = source.claim()?;
        source.seal();
        drop(read);
        assert!(matches!(source.prepared(), Err(EIO)));
        assert!(matches!(source.claim(), Err(EIO)));
        Ok(())
    }

    #[test]
    fn cpu_release_resolves_without_a_native_fence() -> Result {
        let source = Source::new(1)?;
        let read = source.claim()?;
        assert!(matches!(source.claim(), Err(EAGAIN)));
        source.seal();
        read.release_cpu();
        let prepared = source.prepared()?.ok_or(EINVAL)?;
        assert!(prepared.completion()?.is_none());
        assert!(matches!(source.claim(), Err(EBUSY)));
        Ok(())
    }

    #[test]
    fn unsealed_source_does_not_yield_prepared_owner() -> Result {
        let source = Source::new(1)?;
        source.claim()?.release_cpu();
        assert!(source.prepared()?.is_none());
        source.seal();
        assert!(source.prepared()?.is_some());
        Ok(())
    }
}
