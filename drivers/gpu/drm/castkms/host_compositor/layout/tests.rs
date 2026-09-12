// SPDX-License-Identifier: GPL-2.0-only

use super::*;

#[kunit_tests(rust_castkms_host_layout)]
mod cases {
    use super::*;

    #[test]
    fn unsupported_dimensions_are_rejected_without_allocations() {
        for (width, height) in [(0, 1), (1, 0), (1921, 1), (1, 1081), (u32::MAX, u32::MAX)] {
            assert_eq!(Layout::new(width, height), Err(EINVAL));
        }
    }

    #[test]
    fn packed_rows_fit_inside_a_page_rounded_allocation() -> Result {
        for (width, height) in [(1, 1), (3, 2), (640, 480), (1920, 1080)] {
            let layout = Layout::new(width, height)?;
            assert_eq!(layout.dimensions(), (width, height));
            assert_eq!(layout.pitch(), width as usize * 4);
            let visible = layout.pitch() * height as usize;
            assert!(layout.size() >= visible);
            assert!(layout.size() - visible < kernel::page::PAGE_SIZE);
            assert_eq!(layout.size() % kernel::page::PAGE_SIZE, 0);
            assert!(layout.size() <= 8 * 1024 * 1024);
        }
        Ok(())
    }

    #[test]
    fn equal_allocation_sizes_do_not_hide_different_geometry() -> Result {
        let first = Layout::new(3, 2)?;
        let second = Layout::new(2, 3)?;
        assert_eq!(first.size(), second.size());
        assert_ne!(first, second);
        assert_eq!(first, Layout::new(3, 2)?);
        Ok(())
    }
}
