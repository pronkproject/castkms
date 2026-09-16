// SPDX-License-Identifier: GPL-2.0 OR MIT

use super::*;
use crate::drm::fourcc;

#[kunit_tests(rust_drm_constraints_description)]
mod cases {
    use super::*;

    #[test]
    fn description_owns_input_after_caller_changes_it() -> Result {
        let mut formats = [Format::new(
            7,
            fourcc::XRGB8888,
            fourcc::FORMAT_MOD_LINEAR,
            Size::exact(128, 64),
        )];
        let description = Description::new(Size::new(64, 32, 256, 128), &formats)?;
        formats[0] = Format::new(9, fourcc::ARGB8888, 1, Size::exact(32, 16));
        assert_eq!(formats[0].plane_id(), 9);
        assert_eq!(description.output().minimum(), (64, 32));
        assert_eq!(description.output().maximum(), (256, 128));
        let retained = description.clone();
        drop(description);
        let stored = retained.formats();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].plane_id(), 7);
        assert_eq!(stored[0].format(), fourcc::XRGB8888);
        assert_eq!(stored[0].modifier(), fourcc::FORMAT_MOD_LINEAR);
        assert_eq!(stored[0].size().minimum(), (128, 64));
        assert_eq!(stored[0].size().maximum(), (128, 64));
        Ok(())
    }

    #[test]
    fn native_validation_rejects_bad_or_duplicate_records() -> Result {
        let size = Size::exact(128, 64);
        let format = Format::new(7, fourcc::XRGB8888, fourcc::FORMAT_MOD_LINEAR, size);
        assert!(matches!(Description::new(size, &[]), Err(EINVAL)));
        assert!(matches!(
            Description::new(Size::exact(0, 64), &[format]),
            Err(EINVAL)
        ));
        assert!(matches!(
            Description::new(size, &[format, format]),
            Err(EEXIST)
        ));
        assert!(matches!(
            Description::new(size, &[Format::new(7, 0, 0, size)]),
            Err(EINVAL)
        ));
        Ok(())
    }

    #[test]
    fn tiled_layout_retains_independent_exact_geometry() -> Result {
        // Standard Intel X-tiled modifier: vendor 0x01, modifier 1.
        const X_TILED: u64 = (1 << 56) | 1;
        let formats = [
            Format::new(7, fourcc::XRGB8888, 0, Size::exact(128, 64)),
            Format::new(7, fourcc::ARGB8888, X_TILED, Size::exact(256, 128)),
        ];
        let description = Description::new(Size::exact(128, 64), &formats)?;
        assert_eq!(description.formats()[1].modifier(), X_TILED);
        assert_eq!(description.formats()[1].size().minimum(), (256, 128));
        assert_eq!(description.formats()[1].size().maximum(), (256, 128));
        Ok(())
    }
}
