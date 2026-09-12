// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use crate::host_compositor::layout::Layout;
use crate::host_compositor::pool::Pool;

#[kunit_tests(rust_castkms_host_pool)]
mod cases {
    use super::*;

    #[test]
    fn only_two_slots_can_be_reserved_at_once() -> Result {
        let fixture = Fixture::new()?;
        let pool = Pool::new(
            fixture.drm.device(),
            &fixture.host_budget,
            Layout::new(3, 2)?,
        )?;
        let first = pool.reserve()?;
        let second = pool.reserve()?;
        check(matches!(pool.reserve(), Err(EBUSY)))?;
        drop(first);
        let _reused = pool.reserve()?;
        check(matches!(pool.reserve(), Err(EBUSY)))?;
        drop(second);
        let _other = pool.reserve()?;
        Ok(())
    }

    #[test]
    fn private_images_remain_independent_across_reservations() -> Result {
        let fixture = Fixture::new()?;
        let pool = Pool::new(
            fixture.drm.device(),
            &fixture.host_budget,
            Layout::new(3, 2)?,
        )?;
        let mut first = pool.reserve()?;
        let second = pool.reserve()?;
        first.write_row(0, &[0x31; 12])?;
        let mut pixels = [0xff; 12];
        second.with_image(|image| image.read_row(0, &mut pixels))??;
        check(pixels == [0; 12])?;
        drop(first);
        let first = pool.reserve()?;
        first.with_image(|image| image.read_row(0, &mut pixels))??;
        check(pixels == [0x31; 12])?;
        Ok(())
    }

    #[test]
    fn failed_second_image_returns_the_first_images_charge() -> Result {
        let fixture = Fixture::new()?;
        let _remainder = fixture
            .host_budget
            .reserve(16 * 1024 * 1024 - kernel::page::PAGE_SIZE)?;
        for _ in 0..3 {
            check(matches!(
                Pool::new(
                    fixture.drm.device(),
                    &fixture.host_budget,
                    Layout::new(1, 1)?
                ),
                Err(EBUSY)
            ))?;
            let _page = fixture.host_budget.reserve(kernel::page::PAGE_SIZE)?;
        }
        Ok(())
    }

    #[test]
    fn retired_images_remain_charged_until_their_final_release() -> Result {
        let fixture = Fixture::new()?;
        let _remainder = fixture
            .host_budget
            .reserve(16 * 1024 * 1024 - 2 * kernel::page::PAGE_SIZE)?;
        let pool = Pool::new(
            fixture.drm.device(),
            &fixture.host_budget,
            Layout::new(1, 1)?,
        )?;
        let mut retained = pool.reserve()?;
        retained.write_row(0, &[0x42; 4])?;
        pool.close();
        drop(pool);
        check(matches!(
            Pool::new(
                fixture.drm.device(),
                &fixture.host_budget,
                Layout::new(1, 1)?
            ),
            Err(EBUSY)
        ))?;
        let mut pixels = [0; 4];
        retained.with_image(|image| image.read_row(0, &mut pixels))??;
        check(pixels == [0x42; 4])?;
        drop(retained);
        let replacement = Pool::new(
            fixture.drm.device(),
            &fixture.host_budget,
            Layout::new(1, 1)?,
        )?;
        check(matches!(fixture.host_budget.reserve(1), Err(EBUSY)))?;
        replacement.close();
        let _both_pages = fixture.host_budget.reserve(2 * kernel::page::PAGE_SIZE)?;
        Ok(())
    }

    #[test]
    fn shutdown_rejects_reservations_but_retains_active_storage() -> Result {
        let fixture = Fixture::new()?;
        let pool = Pool::new(
            fixture.drm.device(),
            &fixture.host_budget,
            Layout::new(3, 2)?,
        )?;
        let mut slot = pool.reserve()?;
        pool.close();
        pool.close();
        check(matches!(pool.reserve(), Err(ENODEV)))?;
        check(slot.with_image(|image| image.dimensions())? == (3, 2))?;
        drop(pool);
        slot.write_row(0, &[0x42; 12])?;
        drop(slot);
        Ok(())
    }
}
