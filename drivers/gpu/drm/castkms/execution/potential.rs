// SPDX-License-Identifier: GPL-2.0-only

//! Static KMS envelope; the active whole-scene contract remains authoritative.

use kernel::drm::fourcc;
#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
use kernel::prelude::*;

pub(crate) const MAX_DIMENSION: u32 = 16384;
pub(crate) const MAX_CURSOR_DIMENSION: u32 = 512;

/// GPU-only packed formats whose storage is passed through without HOST interpretation.
const PACKED: &[u32] = &[
    fourcc::XRGB4444,
    fourcc::XBGR4444,
    fourcc::RGBX4444,
    fourcc::BGRX4444,
    fourcc::ARGB4444,
    fourcc::ABGR4444,
    fourcc::RGBA4444,
    fourcc::BGRA4444,
    fourcc::XRGB1555,
    fourcc::XBGR1555,
    fourcc::RGBX5551,
    fourcc::BGRX5551,
    fourcc::ARGB1555,
    fourcc::ABGR1555,
    fourcc::RGBA5551,
    fourcc::BGRA5551,
    fourcc::RGBX8888,
    fourcc::BGRX8888,
    fourcc::RGBX1010102,
    fourcc::BGRX1010102,
    fourcc::RGBA1010102,
    fourcc::BGRA1010102,
    fourcc::XRGB16161616F,
    fourcc::XBGR16161616F,
    fourcc::ARGB16161616F,
    fourcc::ABGR16161616F,
];

/// Vendor-neutral framebuffer formats constructible before renderer selection.
pub(crate) const FORMATS: [u32; crate::formats::FORMATS.len() + PACKED.len()] = {
    let mut formats = [0; crate::formats::FORMATS.len() + PACKED.len()];
    let mut index = 0;
    while index < crate::formats::FORMATS.len() {
        formats[index] = crate::formats::FORMATS[index];
        index += 1;
    }
    let mut packed = 0;
    while packed < PACKED.len() {
        formats[index + packed] = PACKED[packed];
        packed += 1;
    }
    formats
};

// DRM's plane format bitmask currently carries at most 64 distinct fourcc values.
const _: () = assert!(FORMATS.len() <= 64);

/// Number of memory planes carried for one renderer-visible framebuffer format.
pub(crate) fn plane_count(format: u32) -> usize {
    if crate::formats::FORMATS.contains(&format) {
        crate::formats::plane_count(format)
    } else if PACKED.contains(&format) {
        1
    } else {
        0
    }
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
#[kunit_tests(rust_castkms_potential_formats)]
mod tests {
    use super::*;

    #[test]
    fn every_format_has_one_unambiguous_plane_count() {
        for (index, format) in FORMATS.iter().enumerate() {
            assert_ne!(*format, 0);
            assert_ne!(plane_count(*format), 0);
            assert!(!FORMATS[..index].contains(format));
        }
        assert_eq!(plane_count(0), 0);
    }

    #[test]
    fn renderer_only_formats_do_not_expand_host_composition() {
        for format in PACKED {
            assert!(!crate::formats::FORMATS.contains(format));
            assert_eq!(plane_count(*format), 1);
        }
        assert_eq!(plane_count(fourcc::NV12), 2);
    }
}
