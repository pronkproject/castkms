// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use crate::host_compositor::image::Image;
use crate::host_compositor::layout::Layout;

#[kunit_tests(rust_castkms_host_images)]
mod cases {
    use super::*;

    #[test]
    fn a_private_image_starts_cleared() -> Result {
        let fixture = Fixture::new()?;
        let image = Image::new(
            fixture.drm.device(),
            &fixture.host_budget,
            Layout::new(3, 2)?,
        )?;
        let mut row = [0xff; 12];
        check(image.dimensions() == (3, 2))?;
        image.read_row(0, &mut row)?;
        check(row == [0; 12])?;
        image.read_row(1, &mut row)?;
        check(row == [0; 12])?;
        Ok(())
    }

    #[test]
    fn rows_are_independent_and_access_is_bounded() -> Result {
        let fixture = Fixture::new()?;
        let mut image = Image::new(
            fixture.drm.device(),
            &fixture.host_budget,
            Layout::new(3, 2)?,
        )?;
        image.write_row(1, &[0x57; 12])?;
        check(image.write_row(0, &[0xff; 11]) == Err(EINVAL))?;
        check(image.write_row(2, &[0xff; 12]) == Err(EINVAL))?;
        let mut row = [0xff; 12];
        image.read_row(0, &mut row)?;
        check(row == [0; 12])?;
        image.read_row(1, &mut row)?;
        check(row == [0x57; 12])?;
        check(image.read_row(u32::MAX, &mut row) == Err(EINVAL))?;
        check(image.read_row(0, &mut [0; 13]) == Err(EINVAL))?;
        Ok(())
    }

    #[test]
    fn full_pixel_copy_excludes_padding_and_owns_independent_bytes() -> Result {
        let fixture = Fixture::new()?;
        let layout = Layout::new(3, 2)?;
        let mut image = Image::new(fixture.drm.device(), &fixture.host_budget, layout)?;
        image.write_row(0, &[0x35; 12])?;
        image.write_row(1, &[0x71; 12])?;
        let mut pixels = [0xff; 24];
        check(layout.pixel_bytes() == pixels.len())?;
        check(layout.size() > pixels.len())?;
        image.copy_pixels(&mut pixels)?;
        image.write_row(0, &[0x99; 12])?;
        drop(image);
        check(pixels[..12] == [0x35; 12])?;
        check(pixels[12..] == [0x71; 12])?;
        Ok(())
    }

    #[test]
    fn full_pixel_copy_rejects_wrong_sizes_before_writing() -> Result {
        let fixture = Fixture::new()?;
        let image = Image::new(
            fixture.drm.device(),
            &fixture.host_budget,
            Layout::new(3, 2)?,
        )?;
        let mut short = [0xa7; 23];
        let mut long = [0xa7; 25];
        check(image.copy_pixels(&mut []) == Err(EINVAL))?;
        check(image.copy_pixels(&mut short) == Err(EINVAL))?;
        check(image.copy_pixels(&mut long) == Err(EINVAL))?;
        check(short == [0xa7; 23])?;
        check(long == [0xa7; 25])?;
        Ok(())
    }

    #[test]
    fn image_accounting_includes_page_padding() -> Result {
        let fixture = Fixture::new()?;
        let remainder = fixture
            .host_budget
            .reserve(crate::host_compositor::budget::LIMIT - kernel::page::PAGE_SIZE)?;
        let image = Image::new(
            fixture.drm.device(),
            &fixture.host_budget,
            Layout::new(1, 1)?,
        )?;
        check(matches!(fixture.host_budget.reserve(1), Err(EBUSY)))?;
        drop(image);
        let _page = fixture.host_budget.reserve(kernel::page::PAGE_SIZE)?;
        drop(remainder);
        Ok(())
    }

    #[test]
    fn dimensions_are_checked_before_allocation() -> Result {
        let fixture = Fixture::new()?;
        for (width, height) in [(0, 1), (1, 0), (1921, 1), (1, 1081), (u32::MAX, u32::MAX)] {
            check(matches!(Layout::new(width, height), Err(EINVAL)))?;
        }
        let image = Image::new(
            fixture.drm.device(),
            &fixture.host_budget,
            Layout::new(1920, 1080)?,
        )?;
        check(image.dimensions() == (1920, 1080))?;
        Ok(())
    }
}
