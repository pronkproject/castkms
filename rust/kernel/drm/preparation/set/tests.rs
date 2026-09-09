// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;
use crate::dma_fence::{testing::ManualFence, Status};
use crate::drm::preparation::Domain;

#[kunit_tests(rust_drm_preparation_set)]
mod cases {
    use super::*;

    #[test]
    fn duplicate_sources_remain_held_until_last_owner() -> Result {
        let domain = Domain::new()?;
        let a = Source::new_in(&domain, 1)?;
        let b = Source::new_in(&domain, 1)?;
        let set = RetirementSet::new(&[a.clone(), b.clone(), a.clone()])?;
        let retained = set.clone();
        drop(set);
        drop(domain);
        assert!(matches!(a.claim(), Err(EBUSY)));
        assert!(matches!(b.claim(), Err(EBUSY)));
        drop(retained);
        a.claim()?.release_cpu();
        b.claim()?.release_cpu();
        Ok(())
    }

    #[test]
    fn failed_construction_leaves_healthy_source_open() -> Result {
        let domain = Domain::new()?;
        let healthy = Source::new_in(&domain, 1)?;
        let failed = Source::new_in(&domain, 1)?;
        drop(failed.claim()?);
        assert!(matches!(
            RetirementSet::new(&[healthy.clone(), failed]),
            Err(EIO)
        ));
        healthy.claim()?.release_cpu();
        Ok(())
    }

    #[test]
    fn mixed_domains_fail_without_holding_either_source() -> Result {
        let a = Source::new(1)?;
        let b = Source::new(1)?;
        assert!(matches!(
            RetirementSet::new(&[a.clone(), b.clone()]),
            Err(EXDEV)
        ));
        a.claim()?.release_cpu();
        b.claim()?.release_cpu();
        Ok(())
    }

    #[test]
    fn overlapping_sets_preserve_the_remaining_owner() -> Result {
        let domain = Domain::new()?;
        let a = Source::new_in(&domain, 1)?;
        let b = Source::new_in(&domain, 1)?;
        let first = RetirementSet::new(&[a.clone(), b.clone()])?;
        let second = RetirementSet::new(&[b.clone()])?;
        drop(first);
        a.claim()?.release_cpu();
        assert!(matches!(b.claim(), Err(EBUSY)));
        drop(second);
        b.claim()?.release_cpu();
        Ok(())
    }

    #[test]
    fn empty_set_has_an_independent_owned_lifetime() -> Result {
        let set = RetirementSet::new(&[])?;
        let retained = set.clone();
        drop(set);
        drop(retained);
        Ok(())
    }

    #[test]
    fn readiness_wait_retains_holds_without_waiting_for_gpu() -> Result {
        let source = Source::new(1)?;
        let read = source.claim()?;
        let set = RetirementSet::new(&[source.clone()])?;
        let mut native = ManualFence::new()?;
        read.release_submitted(&native.fence());
        let prepared = set.wait_prepared()?;
        drop(set);
        let completion = prepared.completion()?.ok_or(EINVAL)?;
        assert_eq!(completion.status(), Status::Pending);
        assert!(matches!(source.claim(), Err(EBUSY)));
        native.complete(Err(EIO))?;
        drop(prepared);
        source.claim()?.release_cpu();
        Ok(())
    }

    #[test]
    fn readiness_wait_reports_abandonment_and_accepts_empty_sets() -> Result {
        let source = Source::new(1)?;
        let read = source.claim()?;
        let set = RetirementSet::new(&[source])?;
        drop(read);
        assert!(matches!(set.wait_prepared(), Err(EIO)));
        let empty = RetirementSet::new(&[])?;
        let ready = empty.wait_prepared()?;
        drop(empty);
        assert!(ready.completion()?.is_none());
        Ok(())
    }

    #[test]
    fn preparation_waits_for_every_claim_and_retains_every_hold() -> Result {
        let domain = Domain::new()?;
        let a = Source::new_in(&domain, 1)?;
        let b = Source::new_in(&domain, 1)?;
        let first = a.claim()?;
        let second = b.claim()?;
        let set = RetirementSet::new(&[a.clone(), b.clone()])?;
        assert!(set.prepared()?.is_none());
        first.release_cpu();
        assert!(set.prepared()?.is_none());
        second.release_cpu();
        let prepared = set.prepared()?.ok_or(EINVAL)?;
        drop(set);
        assert!(matches!(a.claim(), Err(EBUSY)));
        assert!(matches!(b.claim(), Err(EBUSY)));
        assert!(prepared.completion()?.is_none());
        drop(prepared);
        a.claim()?.release_cpu();
        b.claim()?.release_cpu();
        Ok(())
    }

    #[test]
    fn abandonment_wins_over_another_pending_claim() -> Result {
        let domain = Domain::new()?;
        let a = Source::new_in(&domain, 1)?;
        let b = Source::new_in(&domain, 1)?;
        let first = a.claim()?;
        let second = b.claim()?;
        let set = RetirementSet::new(&[a, b])?;
        drop(second);
        assert!(matches!(set.prepared(), Err(EIO)));
        first.release_cpu();
        assert!(matches!(set.prepared(), Err(EIO)));
        Ok(())
    }

    #[test]
    fn completion_outlives_preparation_and_waits_for_both_readers() -> Result {
        let domain = Domain::new()?;
        let a = Source::new_in(&domain, 1)?;
        let b = Source::new_in(&domain, 1)?;
        let first = a.claim()?;
        let second = b.claim()?;
        let mut native_a = ManualFence::new()?;
        let mut native_b = ManualFence::new()?;
        let set = RetirementSet::new(&[a, b])?;
        first.release_submitted(&native_a.fence());
        second.release_submitted(&native_b.fence());
        let prepared = set.prepared()?.ok_or(EINVAL)?;
        let completion = prepared.completion()?.ok_or(EINVAL)?;
        drop(set);
        drop(prepared);
        assert_eq!(completion.status(), Status::Pending);
        native_a.complete(Ok(()))?;
        assert_eq!(completion.status(), Status::Pending);
        native_b.complete(Ok(()))?;
        assert_eq!(completion.status(), Status::Complete(Ok(())));
        Ok(())
    }

    #[test]
    fn empty_preparation_needs_no_fence() -> Result {
        let set = RetirementSet::new(&[])?;
        let prepared = set.prepared()?.ok_or(EINVAL)?;
        drop(set);
        assert!(prepared.completion()?.is_none());
        Ok(())
    }

    #[test]
    fn shared_native_completion_covers_both_sources() -> Result {
        let domain = Domain::new()?;
        let a = Source::new_in(&domain, 1)?;
        let b = Source::new_in(&domain, 1)?;
        let first = a.claim()?;
        let second = b.claim()?;
        let mut native = ManualFence::new()?;
        let set = RetirementSet::new(&[a, b])?;
        first.release_submitted(&native.fence());
        second.release_submitted(&native.fence());
        let prepared = set.prepared()?.ok_or(EINVAL)?;
        let completion = prepared.completion()?.ok_or(EINVAL)?;
        assert_eq!(completion.status(), Status::Pending);
        native.complete(Ok(()))?;
        assert_eq!(completion.status(), Status::Complete(Ok(())));
        Ok(())
    }
}
