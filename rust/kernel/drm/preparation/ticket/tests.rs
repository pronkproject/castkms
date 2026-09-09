// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;
use crate::drm::preparation::Source;

#[kunit_tests(rust_drm_preparation_ticket)]
mod cases {
    use super::*;

    #[test]
    fn final_ticket_reference_owns_the_set() -> Result {
        let source = Source::new(1)?;
        let set = RetirementSet::new(&[source.clone()])?;
        let ticket = Ticket::new(&set)?;
        let retained = ticket.clone();
        drop(set);
        drop(ticket);
        assert!(matches!(source.claim(), Err(EBUSY)));
        drop(retained);
        source.claim()?.release_cpu();
        Ok(())
    }

    #[test]
    fn cancellation_releases_only_ticket_ownership() -> Result {
        let source = Source::new(1)?;
        let set = RetirementSet::new(&[source.clone()])?;
        let ticket = Ticket::new(&set)?;
        ticket.cancel();
        ticket.cancel();
        assert!(matches!(source.claim(), Err(EBUSY)));
        drop(set);
        source.claim()?.release_cpu();
        drop(ticket);
        source.claim()?.release_cpu();
        Ok(())
    }
}
