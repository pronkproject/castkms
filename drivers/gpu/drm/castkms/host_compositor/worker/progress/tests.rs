// SPDX-License-Identifier: GPL-2.0-only

use super::*;

fn check(condition: bool) -> Result {
    if condition {
        Ok(())
    } else {
        Err(EINVAL)
    }
}

#[kunit_tests(rust_castkms_host_progress)]
mod cases {
    use super::*;

    #[test]
    fn one_attempt_covers_all_requests_present_at_its_start() -> Result {
        let mut progress = Progress::new();
        let first = progress.request()?;
        let second = progress.request()?;
        check(progress.observe(first).is_none())?;
        let through = progress.starting();
        check(progress.finish(through, Outcome::NoScene).is_none())?;
        check(matches!(progress.observe(first), Some(Outcome::NoScene)))?;
        check(matches!(progress.observe(second), Some(Outcome::NoScene)))?;
        check(matches!(progress.observe(first), Some(Outcome::NoScene)))?;
        Ok(())
    }

    #[test]
    fn a_request_during_execution_needs_a_later_attempt() -> Result {
        let mut progress = Progress::new();
        let first = progress.request()?;
        let through = progress.starting();
        let second = progress.request()?;
        progress.finish(through, Outcome::NoScene);
        check(matches!(progress.observe(first), Some(Outcome::NoScene)))?;
        check(progress.observe(second).is_none())?;
        let through = progress.starting();
        progress.finish(through, Outcome::Failed(EIO));
        check(matches!(
            progress.observe(second),
            Some(Outcome::Failed(EIO))
        ))?;
        Ok(())
    }

    #[test]
    fn an_old_completion_does_not_satisfy_a_new_request() -> Result {
        let mut progress = Progress::new();
        let first = progress.request()?;
        progress.finish(progress.starting(), Outcome::NoScene);
        let second = progress.request()?;
        check(progress.observe(second).is_none())?;
        check(matches!(progress.observe(first), Some(Outcome::NoScene)))?;
        Ok(())
    }

    #[test]
    fn request_overflow_preserves_the_last_valid_attempt() -> Result {
        let mut progress = Progress {
            requested: u64::MAX - 1,
            completed: None,
        };
        let last = progress.request()?;
        check(last == u64::MAX)?;
        check(progress.request() == Err(EOVERFLOW))?;
        check(progress.starting() == last)?;
        progress.finish(last, Outcome::NoScene);
        check(matches!(progress.observe(last), Some(Outcome::NoScene)))?;
        check(progress.request() == Err(EOVERFLOW))?;
        Ok(())
    }
}
