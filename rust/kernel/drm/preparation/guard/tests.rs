// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;
use crate::{
    dma_fence::{
        testing::ManualFence,
        Status, //
    },
    drm::preparation::{
        Domain,
        RetirementSet,
        Source, //
    },
    sync::aref::ARef, //
};

#[kunit_tests(rust_drm_retirement_guard)]
mod cases {
    use super::*;

    #[test]
    fn transfer_retains_admission_after_preparation_is_dropped() -> Result {
        let domain = Domain::new()?;
        let a = Source::new_in(&domain, 1)?;
        let b = Source::new_in(&domain, 1)?;
        let set = RetirementSet::new(&[a.clone(), b.clone()])?;
        let prepared = set.prepared()?.ok_or(EINVAL)?;
        let guard = RetirementGuard::new(&prepared)?;
        drop(prepared);
        drop(set);
        let mut destination = None;
        assert!(matches!(a.claim(), Err(EBUSY)));
        assert!(matches!(b.claim(), Err(EBUSY)));
        assert!(destination.replace(guard).is_none());
        assert!(matches!(a.claim(), Err(EBUSY)));
        assert!(matches!(b.claim(), Err(EBUSY)));
        assert!(destination.as_ref().ok_or(EINVAL)?.completion().is_none());
        drop(destination);
        a.claim()?.release_cpu();
        b.claim()?.release_cpu();
        Ok(())
    }

    #[test]
    fn abandoned_attempt_leaves_preparation_retryable() -> Result {
        let source = Source::new(1)?;
        let set = RetirementSet::new(&[source.clone()])?;
        let prepared = set.prepared()?.ok_or(EINVAL)?;
        let first = RetirementGuard::new(&prepared)?;
        drop(first);
        assert!(matches!(source.claim(), Err(EBUSY)));
        let retry = RetirementGuard::new(&prepared)?;
        drop(set);
        drop(prepared);
        assert!(matches!(source.claim(), Err(EBUSY)));
        drop(retry);
        source.claim()?.release_cpu();
        Ok(())
    }

    #[test]
    fn borrowed_completion_stays_identical_after_native_signal() -> Result {
        let source = Source::new(1)?;
        let read = source.claim()?;
        let mut native = ManualFence::new()?;
        let set = RetirementSet::new(&[source])?;
        read.release_submitted(&native.fence());
        let prepared = set.prepared()?.ok_or(EINVAL)?;
        let guard = RetirementGuard::new(&prepared)?;
        drop(prepared);
        drop(set);
        let before = guard.completion().ok_or(EINVAL)?;
        assert_eq!(before.status(), Status::Pending);
        native.complete(Ok(()))?;
        let after = guard.completion().ok_or(EINVAL)?;
        assert!(core::ptr::eq(before, after));
        assert_eq!(after.status(), Status::Complete(Ok(())));
        Ok(())
    }

    #[test]
    fn retained_fence_survives_guard_without_owning_admission() -> Result {
        let source = Source::new(2)?;
        let read = source.claim()?;
        let mut native = ManualFence::new()?;
        let set = RetirementSet::new(&[source.clone()])?;
        read.release_submitted(&native.fence());
        let prepared = set.prepared()?.ok_or(EINVAL)?;
        let guard = RetirementGuard::new(&prepared)?;
        let completion: ARef<Fence> = guard.completion().ok_or(EINVAL)?.into();
        drop(prepared);
        drop(set);
        drop(guard);
        source.claim()?.release_cpu();
        assert_eq!(completion.status(), Status::Pending);
        native.complete(Ok(()))?;
        assert_eq!(completion.status(), Status::Complete(Ok(())));
        Ok(())
    }

    #[test]
    fn empty_guard_can_be_moved_without_native_completion() -> Result {
        let set = RetirementSet::new(&[])?;
        let prepared = set.prepared()?.ok_or(EINVAL)?;
        let guard = RetirementGuard::new(&prepared)?;
        drop(prepared);
        drop(set);
        let destination = Some(guard);
        assert!(destination.as_ref().ok_or(EINVAL)?.completion().is_none());
        Ok(())
    }
}
