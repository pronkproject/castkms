// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;
use crate::drm::preparation::Source;

#[kunit_tests(rust_drm_preparation_domain)]
mod cases {
    use super::*;

    #[test]
    fn sources_retain_domain_after_creator_release() -> Result {
        let domain = Domain::new()?;
        let source = Source::new_in(&domain, 1)?;
        let second = Source::new_in(&domain, 1)?;
        drop(domain);
        source.claim()?.release_cpu();
        second.claim()?.release_cpu();
        Ok(())
    }

    #[test]
    fn invalid_capacity_does_not_consume_domain() -> Result {
        let domain = Domain::new()?;
        assert!(matches!(Source::new_in(&domain, 0), Err(EINVAL)));
        Source::new_in(&domain, 1)?.claim()?.release_cpu();
        Ok(())
    }
}
