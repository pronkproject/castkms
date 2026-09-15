// SPDX-License-Identifier: GPL-2.0-only

//! Source pixel storage and conversion to the host's opaque RGB image.

use kernel::{drm::fourcc::*, prelude::*};

pub(crate) const FORMATS: &[u32] = &[
    XRGB8888,
    ARGB8888,
    XBGR8888,
    ABGR8888,
    RGBA8888,
    BGRA8888,
    XRGB2101010,
    ARGB2101010,
    XBGR2101010,
    ABGR2101010,
    RGB888,
    BGR888,
    RGB565,
    BGR565,
    XRGB16161616,
    ARGB16161616,
    XBGR16161616,
    ABGR16161616,
];

/// Sampling factors and stored bits per sample in one format plane.
#[derive(Clone, Copy)]
pub(crate) struct Plane {
    pub(crate) hsub: usize,
    pub(crate) vsub: usize,
    pub(crate) bits: usize,
}

impl Plane {
    pub(crate) fn row_bytes(self, width: u32) -> usize {
        (width as usize)
            .div_ceil(self.hsub)
            .saturating_mul(self.bits)
            .div_ceil(8)
    }

    pub(crate) fn rows(self, height: u32) -> usize {
        (height as usize).div_ceil(self.vsub)
    }
}

pub(crate) fn plane_count(_: u32) -> usize {
    1
}

pub(crate) fn plane(format: u32, index: usize) -> Result<Plane> {
    if index >= plane_count(format) {
        return Err(EINVAL);
    }
    let bits = match format {
        XRGB8888 | ARGB8888 | XBGR8888 | ABGR8888 | RGBA8888 | BGRA8888 | XRGB2101010
        | ARGB2101010 | XBGR2101010 | ABGR2101010 => 32,
        RGB888 | BGR888 => 24,
        RGB565 | BGR565 => 16,
        XRGB16161616 | ARGB16161616 | XBGR16161616 | ABGR16161616 => 64,
        R1 => 1,
        R2 => 2,
        R4 => 4,
        R8 => 8,
        _ => return Err(EINVAL),
    };
    Ok(Plane {
        hsub: 1,
        vsub: 1,
        bits,
    })
}

/// Decode a pixel using bounded reads at (plane, byte within row, row).
///
/// Alpha is ignored for the opaque primary plane.
pub(crate) fn pixel(
    format: u32,
    x: usize,
    y: usize,
    mut read: impl FnMut(usize, usize, usize, &mut [u8]) -> Result,
) -> Result<u32> {
    let rgb = |r: u32, g: u32, b: u32| (r << 16) | (g << 8) | b;
    let bits = plane(format, 0)?.bits;
    let mut bytes = [0; 8];
    read(0, x * bits / 8, y, &mut bytes[..bits.div_ceil(8)])?;
    let word = u64::from_le_bytes(bytes);
    if format == XRGB8888 {
        return Ok(word as u32);
    }
    let scale = |v: u64, bits: u32| -> u32 {
        let max = (1 << bits) - 1;
        ((v * 255 + max / 2) / max) as u32
    };
    let (r, g, b) = match format {
        XRGB8888 | ARGB8888 | RGB888 => (bytes[2] as u32, bytes[1] as u32, bytes[0] as u32),
        XBGR8888 | ABGR8888 | BGR888 => (bytes[0] as u32, bytes[1] as u32, bytes[2] as u32),
        RGBA8888 => (bytes[3] as u32, bytes[2] as u32, bytes[1] as u32),
        BGRA8888 => (bytes[1] as u32, bytes[2] as u32, bytes[3] as u32),
        XRGB2101010 | ARGB2101010 => (
            scale((word >> 20) & 1023, 10),
            scale((word >> 10) & 1023, 10),
            scale(word & 1023, 10),
        ),
        XBGR2101010 | ABGR2101010 => (
            scale(word & 1023, 10),
            scale((word >> 10) & 1023, 10),
            scale((word >> 20) & 1023, 10),
        ),
        RGB565 => (
            scale((word >> 11) & 31, 5),
            scale((word >> 5) & 63, 6),
            scale(word & 31, 5),
        ),
        BGR565 => (
            scale(word & 31, 5),
            scale((word >> 5) & 63, 6),
            scale((word >> 11) & 31, 5),
        ),
        XRGB16161616 | ARGB16161616 => (
            scale((word >> 32) & 65535, 16),
            scale((word >> 16) & 65535, 16),
            scale(word & 65535, 16),
        ),
        XBGR16161616 | ABGR16161616 => (
            scale(word & 65535, 16),
            scale((word >> 16) & 65535, 16),
            scale((word >> 32) & 65535, 16),
        ),
        R1 | R2 | R4 | R8 => {
            let level = (bytes[0] >> (8 - bits - (x * bits % 8))) as u64 & ((1 << bits) - 1);
            let gray = scale(level, bits as u32);
            (gray, gray, gray)
        }
        _ => return Err(EINVAL),
    };
    Ok(rgb(r, g, b))
}
