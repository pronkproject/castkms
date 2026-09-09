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

    #[test]
    fn abandoned_attempt_allows_exclusive_retry() -> Result {
        let source = Source::new(1)?;
        let set = RetirementSet::new(&[source.clone()])?;
        let ticket = Ticket::new(&set)?;
        let attempt = ticket.reserve()?;
        assert!(matches!(ticket.reserve(), Err(EBUSY)));
        drop(attempt);
        let retry = ticket.reserve()?;
        assert!(matches!(ticket.reserve(), Err(EBUSY)));
        drop(retry);
        drop(ticket);
        drop(set);
        source.claim()?.release_cpu();
        Ok(())
    }

    #[test]
    fn pending_claim_prevents_reservation_until_release() -> Result {
        let source = Source::new(1)?;
        let claim = source.claim()?;
        let set = RetirementSet::new(&[source.clone()])?;
        let ticket = Ticket::new(&set)?;
        assert!(matches!(ticket.reserve(), Err(EAGAIN)));
        claim.release_cpu();
        let attempt = ticket.reserve()?;
        drop(attempt);
        Ok(())
    }

    #[test]
    fn unresolved_dropped_claim_fails_reservation() -> Result {
        let source = Source::new(1)?;
        let claim = source.claim()?;
        let set = RetirementSet::new(&[source.clone()])?;
        let ticket = Ticket::new(&set)?;
        drop(claim);
        assert!(matches!(ticket.reserve(), Err(EIO)));
        Ok(())
    }

    #[test]
    fn canceled_attempt_retains_admission_until_drop() -> Result {
        let source = Source::new(1)?;
        let set = RetirementSet::new(&[source.clone()])?;
        let ticket = Ticket::new(&set)?;
        let attempt = ticket.reserve()?;
        ticket.cancel();
        assert!(matches!(ticket.reserve(), Err(ECANCELED)));
        drop(set);
        drop(ticket);
        assert!(matches!(source.claim(), Err(EBUSY)));
        drop(attempt);
        source.claim()?.release_cpu();
        Ok(())
    }

    #[test]
    fn moved_attempt_outlives_external_ticket_references() -> Result {
        let source = Source::new(1)?;
        let set = RetirementSet::new(&[source.clone()])?;
        let ticket = Ticket::new(&set)?;
        let attempt = ticket.reserve()?;
        drop(set);
        drop(ticket);
        let retained = attempt;
        assert!(matches!(source.claim(), Err(EBUSY)));
        drop(retained);
        source.claim()?.release_cpu();
        Ok(())
    }
}
