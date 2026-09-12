// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use crate::host_compositor::image::Image;

#[kunit_tests(rust_castkms_host_images)]
mod cases {
    use super::*;

    #[test]
    fn a_private_image_starts_cleared() -> Result {
        let fixture = Fixture::new()?;
        let image = Image::new(fixture.drm.device(), 3, 2)?;
        let mut row = [0xff; 12];
        assert_eq!(image.dimensions(), (3, 2));
        image.read_row(0, &mut row)?;
        assert_eq!(row, [0; 12]);
        image.read_row(1, &mut row)?;
        assert_eq!(row, [0; 12]);
        Ok(())
    }

    #[test]
    fn rows_are_independent_and_access_is_bounded() -> Result {
        let fixture = Fixture::new()?;
        let mut image = Image::new(fixture.drm.device(), 3, 2)?;
        image.write_row(1, &[0x57; 12])?;
        assert_eq!(image.write_row(0, &[0xff; 11]), Err(EINVAL));
        assert_eq!(image.write_row(2, &[0xff; 12]), Err(EINVAL));
        let mut row = [0xff; 12];
        image.read_row(0, &mut row)?;
        assert_eq!(row, [0; 12]);
        image.read_row(1, &mut row)?;
        assert_eq!(row, [0x57; 12]);
        assert_eq!(image.read_row(u32::MAX, &mut row), Err(EINVAL));
        assert_eq!(image.read_row(0, &mut [0; 13]), Err(EINVAL));
        Ok(())
    }

    #[test]
    fn dimensions_are_checked_before_allocation() -> Result {
        let fixture = Fixture::new()?;
        for (width, height) in [(0, 1), (1, 0), (1921, 1), (1, 1081), (u32::MAX, u32::MAX)] {
            assert!(matches!(
                Image::new(fixture.drm.device(), width, height),
                Err(EINVAL)
            ));
        }
        let image = Image::new(fixture.drm.device(), 1920, 1080)?;
        assert_eq!(image.dimensions(), (1920, 1080));
        Ok(())
    }
}
