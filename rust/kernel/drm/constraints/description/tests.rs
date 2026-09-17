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
        let description = Description::new(Size::new(64, 32, 256, 128), &formats, &[], &[])?;
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
        assert_eq!(stored[0].modifier(), Some(fourcc::FORMAT_MOD_LINEAR));
        assert_eq!(stored[0].size().minimum(), (128, 64));
        assert_eq!(stored[0].size().maximum(), (128, 64));
        Ok(())
    }

    #[test]
    fn implicit_and_explicit_linear_keep_independent_bounds() -> Result {
        let output = Size::exact(128, 64);
        let formats = [
            Format::new(7, fourcc::XRGB8888, fourcc::FORMAT_MOD_LINEAR, output),
            Format::implicit(7, fourcc::XRGB8888, Size::exact(256, 128)),
        ];
        let description = Description::new(output, &formats, &[], &[])?;
        let stored = description.formats();
        assert_eq!(stored.len(), 2);
        assert_eq!(stored[0].modifier(), Some(fourcc::FORMAT_MOD_LINEAR));
        assert_eq!(stored[1].modifier(), None);
        assert_eq!(stored[0].size().minimum(), (128, 64));
        assert_eq!(stored[1].size().minimum(), (256, 128));
        assert!(matches!(
            Description::new(output, &[formats[1], formats[1]], &[], &[]),
            Err(EEXIST)
        ));
        Ok(())
    }

    #[test]
    fn storage_requirements_are_retained() -> Result {
        let size = Size::exact(256, 128);
        let formats = [Format::new(7, fourcc::NV12, 1, size)
            .with_dimension_alignment(64, 4)
            .with_storage(false, true, 256, 4096, 65536)];
        let description = Description::new(size, &formats, &[], &[])?;
        let format = &description.formats()[0];
        assert!(!format.permits_native());
        assert!(format.permits_imported());
        assert_eq!(format.dimension_alignment(), (64, 4));
        assert_eq!(format.storage_layout(), (256, 4096, 65536));
        Ok(())
    }

    #[test]
    fn malformed_storage_requirements_are_rejected() {
        let size = Size::exact(256, 128);
        for format in [
            Format::new(7, fourcc::XRGB8888, 0, size).with_dimension_alignment(0, 1),
            Format::new(7, fourcc::XRGB8888, 0, size).with_dimension_alignment(3, 1),
            Format::new(7, fourcc::XRGB8888, 0, Size::new(1, 1, 63, 63))
                .with_dimension_alignment(64, 1),
            Format::new(7, fourcc::XRGB8888, 0, size)
                .with_storage(false, false, 1, 1, 4),
            Format::new(7, fourcc::XRGB8888, 0, size)
                .with_storage(true, false, 3, 1, 4),
            Format::new(7, fourcc::XRGB8888, 0, size)
                .with_storage(true, false, 1, 0, 4),
            Format::new(7, fourcc::XRGB8888, 0, size)
                .with_storage(true, false, 8, 1, 4),
        ] {
            assert!(matches!(
                Description::new(size, &[format], &[], &[]),
                Err(EINVAL)
            ));
        }
    }

    #[test]
    fn native_validation_rejects_bad_or_duplicate_records() -> Result {
        let size = Size::exact(128, 64);
        let format = Format::new(7, fourcc::XRGB8888, fourcc::FORMAT_MOD_LINEAR, size);
        assert!(matches!(Description::new(size, &[], &[], &[]), Err(EINVAL)));
        assert!(matches!(
            Description::new(Size::exact(0, 64), &[format], &[], &[]),
            Err(EINVAL)
        ));
        assert!(matches!(
            Description::new(size, &[format, format], &[], &[]),
            Err(EEXIST)
        ));
        assert!(matches!(
            Description::new(size, &[Format::new(7, 0, 0, size)], &[], &[]),
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
        let description = Description::new(Size::exact(128, 64), &formats, &[], &[])?;
        assert_eq!(description.formats()[1].modifier(), Some(X_TILED));
        assert_eq!(description.formats()[1].size().minimum(), (256, 128));
        assert_eq!(description.formats()[1].size().maximum(), (256, 128));
        Ok(())
    }

    #[test]
    fn property_views_preserve_native_scalar_semantics() -> Result {
        let size = Size::exact(128, 64);
        let formats = [Format::new(7, fourcc::XRGB8888, 0, size)];
        let mut rules = [
            Property::unsigned_range(7, 23, 32768, 65535),
            Property::signed_range(7, 24, -4, 4),
            Property::enum_values(7, 25, 1 << 63),
            Property::bitmask(7, 26, 3),
        ];
        let description = Description::new(size, &formats, &rules, &[])?;
        rules[0] = Property::unsigned_range(7, 23, 0, 1);
        assert!(rules[0].matches(1));
        let stored = description.properties();
        assert_eq!(stored.len(), 4);
        assert_eq!(stored[0].object_id(), 7);
        assert_eq!(stored[0].property_id(), 23);
        assert_eq!(stored[0].property_type(), bindings::DRM_MODE_PROP_RANGE);
        assert_eq!(stored[0].bounds(), (32768, 65535));
        assert!(!stored[0].matches(1));
        assert!(stored[1].matches((-4i64) as u64));
        assert!(!stored[1].matches((-5i64) as u64));
        assert_eq!(stored[2].mask(), 1 << 63);
        assert!(stored[2].matches(63));
        assert!(!stored[2].matches(64));
        assert!(stored[3].matches(3));
        assert!(!stored[3].matches(4));
        assert!(Description::new(size, &formats, &[], &[])?
            .properties()
            .is_empty());
        Ok(())
    }

    #[test]
    fn native_validation_rejects_invalid_property_input() -> Result {
        let size = Size::exact(128, 64);
        let formats = [Format::new(7, fourcc::XRGB8888, 0, size)];
        let rule = Property::unsigned_range(7, 23, 0, 1);
        assert!(matches!(
            Description::new(size, &formats, &[rule, rule], &[]),
            Err(EEXIST)
        ));
        assert!(matches!(
            Description::new(
                size,
                &formats,
                &[Property::signed_range(7, 23, 1, -1)],
                &[],
            ),
            Err(EINVAL)
        ));
        assert!(matches!(
            Description::new(
                size,
                &formats,
                &[Property::enum_values(7, 23, 0)],
                &[],
            ),
            Err(EINVAL)
        ));
        Ok(())
    }

    #[test]
    fn overlapping_plane_limits_are_owned_and_bounded() -> Result {
        let size = Size::exact(128, 64);
        let formats = [Format::new(7, fourcc::XRGB8888, 0, size)];
        let all = [7, 8, 9];
        let overlays = [8, 9];
        let limits = [PlaneLimit::new(2, &all)?, PlaneLimit::new(1, &overlays)?];
        let description = Description::new(size, &formats, &[], &limits)?;
        let stored = description.plane_limits();
        assert_eq!(stored.len(), 2);
        assert_eq!(stored[0].max_active(), 2);
        assert_eq!(stored[0].plane_ids(), [7, 8, 9]);
        assert_eq!(stored[1].max_active(), 1);
        assert_eq!(stored[1].plane_ids(), [8, 9]);
        assert!(Description::new(size, &formats, &[], &[])?
            .plane_limits()
            .is_empty());
        Ok(())
    }

    #[test]
    fn native_validation_rejects_invalid_plane_limits() -> Result {
        let size = Size::exact(128, 64);
        let formats = [Format::new(7, fourcc::XRGB8888, 0, size)];
        for (maximum, ids, error) in [
            (0, &[7, 8][..], EINVAL),
            (3, &[7, 8][..], EINVAL),
            (1, &[][..], EINVAL),
            (1, &[0][..], EINVAL),
            (1, &[7, 7][..], EEXIST),
        ] {
            let limit = PlaneLimit::new(maximum, ids)?;
            assert!(Description::new(size, &formats, &[], &[limit])
                .is_err_and(|actual| actual == error));
        }
        Ok(())
    }

    #[test]
    fn plane_geometry_is_owned_bounded_and_compared() -> Result {
        let size = Size::exact(128, 64);
        let formats = [Format::new(7, fourcc::XRGB8888, 0, size)];
        let broad = PlaneGeometry::new(7, true, true, true, 1 << 12, 1 << 20);
        let narrow = PlaneGeometry::new(7, false, false, false, 1 << 16, 1 << 16);
        let description = Description::new_with_geometry(size, &formats, &[], &[], &[narrow])?;
        let geometries = description.plane_geometries();
        assert_eq!(geometries.len(), 1);
        assert_eq!(geometries[0].plane_id(), 7);
        assert_eq!(geometries[0].operations(), (false, false, false));
        assert_eq!(geometries[0].scale(), (1 << 16, 1 << 16));

        let broad = Description::new_with_geometry(size, &formats, &[], &[], &[broad])?;
        assert!(broad.covers(&description));
        assert!(!description.covers(&broad));
        assert!(Description::new(size, &formats, &[], &[])?.covers(&broad));
        Ok(())
    }

    #[test]
    fn native_validation_rejects_invalid_plane_geometry() -> Result {
        let size = Size::exact(128, 64);
        let formats = [Format::new(7, fourcc::XRGB8888, 0, size)];
        for geometry in [
            PlaneGeometry::new(0, true, true, true, 1, 2),
            PlaneGeometry::new(7, true, true, true, 0, 2),
            PlaneGeometry::new(7, true, true, true, 2, 1),
        ] {
            assert!(matches!(
                Description::new_with_geometry(size, &formats, &[], &[], &[geometry]),
                Err(EINVAL)
            ));
        }
        let duplicate = PlaneGeometry::new(7, false, false, false, 1 << 16, 1 << 16);
        assert!(matches!(
            Description::new_with_geometry(size, &formats, &[], &[], &[duplicate; 2]),
            Err(EEXIST)
        ));
        Ok(())
    }
}
