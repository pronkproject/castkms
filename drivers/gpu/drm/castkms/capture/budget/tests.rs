// SPDX-License-Identifier: GPL-2.0-only

use super::*;

fn check(condition: bool) -> Result {
    if condition {
        Ok(())
    } else {
        Err(EINVAL)
    }
}

#[kunit_tests(rust_castkms_capture_budget)]
mod cases {
    use super::*;

    #[test]
    fn invalid_capacity_does_not_consume_storage() -> Result {
        let budget = Budget::new()?;
        let layout = Layout::new(4096, 4095)?;
        check(matches!(budget.reserve(layout, 0), Err(EINVAL)))?;
        check(matches!(
            budget.reserve(layout, CAPACITY_LIMIT + 1),
            Err(E2BIG)
        ))?;
        let _full = budget.reserve(layout, CAPACITY_LIMIT)?;
        Ok(())
    }

    #[test]
    fn maximum_capacity_reflects_the_exact_result_layout() -> Result {
        check(maximum_capacity(Layout::new(1920, 1080)?) == CAPACITY_LIMIT)?;
        let largest = Layout::new(8192, 8192)?;
        check(maximum_capacity(largest) == 2)?;
        check(matches!(Budget::new()?.reserve(largest, 3), Err(E2BIG)))
    }

    #[test]
    fn retained_queue_storage_blocks_replacement_without_waiting() -> Result {
        let budget = Budget::new()?;
        let layout = Layout::new(4096, 4095)?;
        let retained = budget.reserve(layout, CAPACITY_LIMIT)?;
        check(matches!(budget.reserve(layout, 1), Err(EBUSY)))?;
        drop(retained);
        let _replacement = budget.reserve(layout, CAPACITY_LIMIT)?;
        Ok(())
    }

    #[test]
    fn small_images_still_have_a_stream_count_limit() -> Result {
        let budget = Budget::new()?;
        let layout = Layout::new(1, 1)?;
        let mut charges = KVec::new();
        for _ in 0..STREAM_LIMIT {
            charges.push(budget.reserve(layout, 1)?, GFP_KERNEL)?;
        }
        check(matches!(budget.reserve(layout, 1), Err(EBUSY)))?;
        drop(charges.pop());
        let replacement = budget.reserve(layout, 1)?;
        check(matches!(budget.reserve(layout, 1), Err(EBUSY)))?;
        drop(replacement);
        drop(charges);
        let _fresh = budget.reserve(layout, CAPACITY_LIMIT)?;
        Ok(())
    }

    #[test]
    fn failed_reservations_do_not_consume_remaining_capacity() -> Result {
        let budget = Budget::new()?;
        let large = Layout::new(4096, 4095)?;
        let retained = budget.reserve(large, CAPACITY_LIMIT)?;
        for _ in 0..32 {
            check(matches!(budget.reserve(large, 1), Err(EBUSY)))?;
        }
        let _small = budget.reserve(Layout::new(1, 1)?, 1)?;
        drop(retained);
        let _replacement = budget.reserve(large, CAPACITY_LIMIT)?;
        Ok(())
    }

    #[test]
    fn a_charge_retains_its_budget_independently_of_the_issuer() -> Result {
        let budget = Budget::new()?;
        let retained = budget.reserve(Layout::new(4096, 4095)?, CAPACITY_LIMIT)?;
        let observer = budget.clone();
        drop(budget);
        check(matches!(
            observer.reserve(Layout::new(4096, 4095)?, 1),
            Err(EBUSY)
        ))?;
        drop(retained);
        let _replacement = observer.reserve(Layout::new(4096, 4095)?, CAPACITY_LIMIT)?;
        Ok(())
    }
}
