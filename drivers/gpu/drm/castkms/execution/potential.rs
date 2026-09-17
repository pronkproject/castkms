// SPDX-License-Identifier: GPL-2.0-only

//! Static KMS envelope; the active whole-scene contract remains authoritative.

use kernel::drm::fourcc;

pub(crate) const MAX_DIMENSION: u32 = 16384;
pub(crate) const MAX_CURSOR_DIMENSION: u32 = 512;

/// Include GPU-only floating-point formats without teaching HOST how to map them.
pub(crate) const FORMATS: [u32; crate::formats::FORMATS.len() + 4] = {
    let mut formats = [0; crate::formats::FORMATS.len() + 4];
    let mut index = 0;
    while index < crate::formats::FORMATS.len() {
        formats[index] = crate::formats::FORMATS[index];
        index += 1;
    }
    formats[index] = fourcc::XRGB16161616F;
    formats[index + 1] = fourcc::XBGR16161616F;
    formats[index + 2] = fourcc::ARGB16161616F;
    formats[index + 3] = fourcc::ABGR16161616F;
    formats
};
