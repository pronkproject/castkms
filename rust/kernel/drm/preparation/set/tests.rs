// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;
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
}
