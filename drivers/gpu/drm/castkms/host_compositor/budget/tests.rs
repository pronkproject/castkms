// SPDX-License-Identifier: GPL-2.0-only

use super::*;

#[kunit_tests(rust_castkms_host_budget)]
mod cases {
    use super::*;

    #[test]
    fn invalid_sizes_do_not_consume_budget() -> Result {
        let budget = Budget::new()?;
        assert!(matches!(budget.reserve(0), Err(EINVAL)));
        assert!(matches!(budget.reserve(LIMIT + 1), Err(E2BIG)));
        assert!(matches!(budget.reserve(usize::MAX), Err(E2BIG)));
        let _all = budget.reserve(LIMIT)?;
        assert!(matches!(budget.reserve(1), Err(EBUSY)));
        Ok(())
    }

    #[test]
    fn reservations_share_the_limit_and_return_their_own_bytes() -> Result {
        let budget = Budget::new()?;
        let first = budget.reserve(LIMIT - 1)?;
        let last = budget.reserve(1)?;
        assert!(matches!(budget.reserve(1), Err(EBUSY)));
        drop(first);
        assert!(matches!(budget.reserve(LIMIT), Err(EBUSY)));
        let replacement = budget.reserve(LIMIT - 1)?;
        drop(last);
        let _last = budget.reserve(1)?;
        drop(replacement);
        let _replacement = budget.reserve(LIMIT - 1)?;
        Ok(())
    }

    #[test]
    fn reservation_keeps_budget_alive_without_its_creator() -> Result {
        let budget = Budget::new()?;
        let charge = budget.reserve(LIMIT)?;
        drop(budget);
        assert!(matches!(charge.budget.reserve(1), Err(EBUSY)));
        let observer = charge.budget.clone();
        drop(charge);
        let _all = observer.reserve(LIMIT)?;
        Ok(())
    }
}
