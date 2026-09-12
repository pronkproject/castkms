// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use crate::host_compositor::pool::Pool;

#[kunit_tests(rust_castkms_host_pool)]
mod cases {
    use super::*;

    #[test]
    fn only_two_slots_can_be_reserved_at_once() -> Result {
        let fixture = Fixture::new()?;
        let pool = Pool::new(fixture.drm.device(), 3, 2)?;
        let first = pool.reserve()?;
        let second = pool.reserve()?;
        assert!(matches!(pool.reserve(), Err(EBUSY)));
        drop(first);
        let _reused = pool.reserve()?;
        assert!(matches!(pool.reserve(), Err(EBUSY)));
        drop(second);
        let _other = pool.reserve()?;
        Ok(())
    }

    #[test]
    fn private_images_remain_independent_across_reservations() -> Result {
        let fixture = Fixture::new()?;
        let pool = Pool::new(fixture.drm.device(), 3, 2)?;
        let mut first = pool.reserve()?;
        let second = pool.reserve()?;
        first.write_row(0, &[0x31; 12])?;
        let mut pixels = [0xff; 12];
        second.with_image(|image| image.read_row(0, &mut pixels))??;
        assert_eq!(pixels, [0; 12]);
        drop(first);
        let first = pool.reserve()?;
        first.with_image(|image| image.read_row(0, &mut pixels))??;
        assert_eq!(pixels, [0x31; 12]);
        Ok(())
    }

    #[test]
    fn shutdown_rejects_reservations_but_retains_active_storage() -> Result {
        let fixture = Fixture::new()?;
        let pool = Pool::new(fixture.drm.device(), 3, 2)?;
        let mut slot = pool.reserve()?;
        pool.close();
        pool.close();
        assert!(matches!(pool.reserve(), Err(ENODEV)));
        assert_eq!(slot.with_image(|image| image.dimensions())?, (3, 2));
        drop(pool);
        slot.write_row(0, &[0x42; 12])?;
        drop(slot);
        Ok(())
    }
}
