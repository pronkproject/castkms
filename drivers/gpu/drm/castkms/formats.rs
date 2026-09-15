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
    R1,
    R2,
    R4,
    R8,
    NV12,
    NV21,
    NV16,
    NV61,
    NV24,
    NV42,
    YUV420,
    YVU420,
    YUV422,
    YVU422,
    YUV444,
    YVU444,
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

/// Chroma geometry shared by all of the supported YUV layouts.
fn yuv(format: u32) -> Option<(usize, usize, bool, bool, usize)> {
    Some(match format {
        NV12 => (2, 2, false, false, 8),
        NV21 => (2, 2, false, true, 8),
        NV16 => (2, 1, false, false, 8),
        NV61 => (2, 1, false, true, 8),
        NV24 => (1, 1, false, false, 8),
        NV42 => (1, 1, false, true, 8),
        YUV420 => (2, 2, true, false, 8),
        YVU420 => (2, 2, true, true, 8),
        YUV422 => (2, 1, true, false, 8),
        YVU422 => (2, 1, true, true, 8),
        YUV444 => (1, 1, true, false, 8),
        YVU444 => (1, 1, true, true, 8),
        _ => return None,
    })
}

pub(crate) fn plane_count(format: u32) -> usize {
    yuv(format).map_or(1, |(_, _, planar, _, _)| if planar { 3 } else { 2 })
}

pub(crate) fn plane(format: u32, index: usize) -> Result<Plane> {
    if index >= plane_count(format) {
        return Err(EINVAL);
    }
    if let Some((hsub, vsub, planar, _, depth)) = yuv(format) {
        let bits = if depth == 8 { 8 } else { 16 };
        return Ok(if index == 0 {
            Plane {
                hsub: 1,
                vsub: 1,
                bits,
            }
        } else {
            Plane {
                hsub,
                vsub,
                bits: if planar { bits } else { bits * 2 },
            }
        });
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
/// Alpha is ignored for the opaque primary plane. YUV uses BT.601 limited range,
/// the default DRM plane color interpretation; no color properties are exposed.
pub(crate) fn pixel(
    format: u32,
    x: usize,
    y: usize,
    mut read: impl FnMut(usize, usize, usize, &mut [u8]) -> Result,
) -> Result<u32> {
    let rgb = |r: u32, g: u32, b: u32| (r << 16) | (g << 8) | b;
    if let Some((hs, vs, planar, swap, _depth)) = yuv(format) {
        let bytes = 1;
        let mut luma = [0; 2];
        let mut chroma = [0; 4];
        read(0, x * bytes, y, &mut luma[..bytes])?;
        if planar {
            read(1, x / hs, y / vs, &mut chroma[..1])?;
            read(2, x / hs, y / vs, &mut chroma[1..2])?;
        } else {
            read(1, (x / hs) * bytes * 2, y / vs, &mut chroma[..bytes * 2])?;
        }
        let sample = |b: &[u8]| -> i64 { i64::from(b[0]) * 256 };
        let yy = sample(&luma) - 16 * 256;
        let a = sample(&chroma[..bytes]) - 128 * 256;
        let b = sample(&chroma[bytes..]) - 128 * 256;
        let (u, v) = if swap { (b, a) } else { (a, b) };
        let channel = |v: i64| ((v + (1 << 23)) >> 24).clamp(0, 255) as u32;
        return Ok(rgb(
            channel(76284 * yy + 104595 * v),
            channel(76284 * yy - 25624 * u - 53281 * v),
            channel(76284 * yy + 132251 * u),
        ));
    }
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
