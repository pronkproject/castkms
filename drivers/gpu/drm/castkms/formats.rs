// SPDX-License-Identifier: GPL-2.0-only

//! Source pixel storage and conversion to the host's opaque RGB image.

use kernel::{drm::fourcc::*, prelude::*};

pub(crate) const FORMATS: &[u32] = &[XRGB8888];

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
        XRGB8888 | ARGB8888 | XBGR8888 | ABGR8888 | RGBA8888 | BGRA8888 => 32,
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
    let (r, g, b) = match format {
        XRGB8888 | ARGB8888 => (bytes[2] as u32, bytes[1] as u32, bytes[0] as u32),
        XBGR8888 | ABGR8888 => (bytes[0] as u32, bytes[1] as u32, bytes[2] as u32),
        RGBA8888 => (bytes[3] as u32, bytes[2] as u32, bytes[1] as u32),
        BGRA8888 => (bytes[1] as u32, bytes[2] as u32, bytes[3] as u32),
        _ => return Err(EINVAL),
    };
    Ok(rgb(r, g, b))
}
