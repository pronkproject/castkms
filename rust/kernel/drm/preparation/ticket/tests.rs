// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;
use crate::drm::preparation::{
    RetirementSet,
    Source, //
};

#[kunit_tests(rust_drm_preparation_ticket)]
mod cases {
    use super::*;

    #[test]
    fn ticket_requires_valid_output_generations() -> Result {
        let source = Source::new(1)?;
        assert!(matches!(OutputGeneration::new(0, &source), Err(EINVAL)));
        let output = OutputGeneration::new(1, &source)?;
        assert!(matches!(Ticket::new(&[output, output]), Err(EINVAL)));
        assert!(matches!(Ticket::new(&[output; 33]), Err(E2BIG)));
        let other = Source::new(1)?;
        assert!(matches!(
            Ticket::new(&[output, OutputGeneration::new(2, &other)?]),
            Err(EXDEV)
        ));
        source.claim()?.release_cpu();
        other.claim()?.release_cpu();
        Ok(())
    }

    #[test]
    fn final_ticket_reference_owns_the_set() -> Result {
        let source = Source::new(1)?;
        let set = RetirementSet::new(&[source.clone()])?;
        let ticket = Ticket::new(&[OutputGeneration::new(1, &source)?])?;
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
        let ticket = Ticket::new(&[OutputGeneration::new(1, &source)?])?;
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
        let ticket = Ticket::new(&[OutputGeneration::new(1, &source)?])?;
        let attempt = ticket.reserve()?;
        assert_eq!(ticket.status(), TicketStatus::Ready);
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
        let ticket = Ticket::new(&[OutputGeneration::new(1, &source)?])?;
        assert_eq!(ticket.status(), TicketStatus::Pending);
        assert!(matches!(ticket.reserve(), Err(EAGAIN)));
        claim.release_cpu();
        assert_eq!(ticket.status(), TicketStatus::Ready);
        let attempt = ticket.reserve()?;
        drop(attempt);
        Ok(())
    }

    #[test]
    fn unresolved_dropped_claim_fails_reservation() -> Result {
        let source = Source::new(1)?;
        let claim = source.claim()?;
        let ticket = Ticket::new(&[OutputGeneration::new(1, &source)?])?;
        drop(claim);
        assert_eq!(ticket.status(), TicketStatus::Failed);
        assert!(matches!(ticket.reserve(), Err(EIO)));
        Ok(())
    }

    #[test]
    fn canceled_attempt_retains_admission_until_drop() -> Result {
        let source = Source::new(1)?;
        let set = RetirementSet::new(&[source.clone()])?;
        let ticket = Ticket::new(&[OutputGeneration::new(1, &source)?])?;
        let attempt = ticket.reserve()?;
        ticket.cancel();
        assert_eq!(ticket.status(), TicketStatus::Canceled);
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
        let ticket = Ticket::new(&[OutputGeneration::new(1, &source)?])?;
        let attempt = ticket.reserve()?;
        drop(set);
        drop(ticket);
        let retained = attempt;
        assert!(matches!(source.claim(), Err(EBUSY)));
        drop(retained);
        source.claim()?.release_cpu();
        Ok(())
    }

    #[test]
    fn readiness_observation_does_not_reserve_or_override_cancellation() -> Result {
        let source = Source::new(1)?;
        let set = RetirementSet::new(&[source.clone()])?;
        let ticket = Ticket::new(&[OutputGeneration::new(1, &source)?])?;
        ticket.wait_ready()?;
        let attempt = ticket.reserve()?;
        ticket.wait_ready()?;
        assert!(matches!(ticket.reserve(), Err(EBUSY)));
        ticket.cancel();
        assert_eq!(ticket.wait_ready(), Err(ECANCELED));
        drop(set);
        drop(attempt);
        source.claim()?.release_cpu();
        Ok(())
    }

    #[test]
    fn readiness_observation_reports_failed_claims_and_empty_scope() -> Result {
        let source = Source::new(1)?;
        let read = source.claim()?;
        let ticket = Ticket::new(&[OutputGeneration::new(1, &source)?])?;
        drop(read);
        assert_eq!(ticket.wait_ready(), Err(EIO));
        ticket.cancel();
        assert_eq!(ticket.wait_ready(), Err(ECANCELED));
        Ticket::new(&[])?.wait_ready()?;
        Ok(())
    }

    #[test]
    fn native_completion_error_does_not_fail_submission_preparation() -> Result {
        let source = Source::new(1)?;
        let claim = source.claim()?;
        let ticket = Ticket::new(&[OutputGeneration::new(1, &source)?])?;
        let mut native = crate::dma_fence::testing::ManualFence::new()?;
        assert_eq!(ticket.status(), TicketStatus::Pending);
        claim.release_submitted(&native.fence());
        assert_eq!(ticket.status(), TicketStatus::Ready);
        native.complete(Err(EIO))?;
        assert_eq!(ticket.status(), TicketStatus::Ready);
        let attempt = ticket.reserve()?;
        assert_eq!(ticket.status(), TicketStatus::Ready);
        drop(attempt);
        ticket.cancel();
        assert_eq!(ticket.status(), TicketStatus::Canceled);
        Ok(())
    }
}
