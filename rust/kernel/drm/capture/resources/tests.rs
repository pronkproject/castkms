// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;
use core::cell::Cell;

fn check(condition: bool) -> Result {
    if condition {
        Ok(())
    } else {
        Err(EINVAL)
    }
}

struct Counted<'a>(&'a Cell<u32>);

impl Drop for Counted<'_> {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

#[kunit_tests(rust_drm_capture_resources)]
mod cases {
    use super::*;

    #[test]
    fn checking_does_not_reserve_a_name_or_capacity() -> Result {
        let mut table = Resources::new(1)?;
        check(table.check(0) == Err(EINVAL))?;
        table.check(8)?;
        table.check(8)?;
        table.insert(9, || Ok(3))?;
        check(table.check(8) == Err(ESTALE))?;
        check(table.check(10) == Err(EBUSY))?;
        check(table.insert(8, || Ok(4)) == Err(ESTALE))?;
        table.remove(9)?;
        table.check(10)?;
        table.insert(10, || Ok(5))?;
        check(*table.get(10)? == 5)
    }

    #[test]
    fn invalid_names_and_capacity_do_not_construct_resources() -> Result {
        check(matches!(Resources::<u32>::new(0), Err(EINVAL)))?;
        let calls = Cell::new(0);
        let create = || {
            calls.set(calls.get() + 1);
            Ok(7)
        };
        let mut table = Resources::new(1)?;
        check(table.insert(0, create) == Err(EINVAL))?;
        table.insert(10, create)?;
        check(table.insert(10, create) == Err(ESTALE))?;
        check(table.insert(9, create) == Err(ESTALE))?;
        check(table.insert(20, create) == Err(EBUSY))?;
        check(calls.get() == 1)?;
        check(table.remove(10)? == 7)?;
        table.insert(20, create)?;
        check(calls.get() == 2)
    }

    #[test]
    fn failed_construction_leaves_the_name_retryable() -> Result {
        let mut table = Resources::new(2)?;
        check(table.insert(19, || Err(ENOMEM)) == Err(ENOMEM))?;
        table.insert(19, || Ok(4))?;
        *table.get_mut(19)? = 8;
        check(table.get_mut(0) == Err(EINVAL))?;
        check(table.get_mut(20) == Err(ENOENT))?;
        check(table.remove(19)? == 8)?;
        check(table.remove(19) == Err(ENOENT))?;
        check(table.remove(0) == Err(EINVAL))?;
        check(table.insert(19, || Ok(5)) == Err(ESTALE))
    }

    #[test]
    fn exhaustion_preserves_cleanup_but_never_reuses_a_name() -> Result {
        let mut table = Resources::new(2)?;
        table.insert(u64::MAX, || Ok(7))?;
        check(*table.get_mut(u64::MAX)? == 7)?;
        check(table.insert(1, || Ok(8)) == Err(EOVERFLOW))?;
        check(table.remove(u64::MAX)? == 7)?;
        check(table.insert(1, || Ok(8)) == Err(EOVERFLOW))
    }

    #[test]
    fn removal_transfers_cleanup_and_drop_releases_every_remaining_resource() -> Result {
        let drops = Cell::new(0);
        let mut table = Resources::new(2)?;
        table.insert(1, || Ok(Counted(&drops)))?;
        table.insert(2, || Ok(Counted(&drops)))?;
        let removed = table.remove(1)?;
        check(drops.get() == 0)?;
        drop(table);
        check(drops.get() == 1)?;
        drop(removed);
        check(drops.get() == 2)
    }
}
