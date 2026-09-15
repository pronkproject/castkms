// SPDX-License-Identifier: GPL-2.0-only

//! Source pixel storage and conversion to the host's opaque RGB image.

use kernel::drm::kms::plane::{ColorEncoding, ColorRange};
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
    P010,
    P012,
    P016,
];

/// Read per-pixel alpha, normalized to sixteen bits. Formats without alpha are opaque.
pub(crate) fn alpha16(
    format: u32,
    x: usize,
    y: usize,
    mut read: impl FnMut(usize, usize, usize, &mut [u8]) -> Result,
) -> Result<u32> {
    let (bytes, shift, bits) = match format {
        ARGB8888 | ABGR8888 => (4, 24, 8),
        RGBA8888 | BGRA8888 => (4, 0, 8),
        ARGB2101010 | ABGR2101010 => (4, 30, 2),
        ARGB16161616 | ABGR16161616 => (8, 48, 16),
        _ => return Ok(65535),
    };
    let mut sample = [0; 8];
    read(0, x * bytes, y, &mut sample[..bytes])?;
    let maximum = (1u64 << bits) - 1;
    let alpha = (u64::from_le_bytes(sample) >> shift) & maximum;
    Ok(((alpha * 65535 + maximum / 2) / maximum) as u32)
}

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
        P010 => (2, 2, false, false, 10),
        P012 => (2, 2, false, false, 12),
        P016 => (2, 2, false, false, 16),
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
/// a fixed interpretation used by the packed-pixel compatibility helper.
pub(crate) fn pixel(
    format: u32,
    x: usize,
    y: usize,
    mut read: impl FnMut(usize, usize, usize, &mut [u8]) -> Result,
) -> Result<u32> {
    // Preserve the unused byte for the opaque packed-copy path.
    if format == XRGB8888 {
        let mut bytes = [0; 4];
        read(0, x * 4, y, &mut bytes)?;
        return Ok(u32::from_le_bytes(bytes));
    }
    let [r, g, b] = channels(format, x, y, 255, None, read)?;
    Ok((r << 16) | (g << 8) | b)
}

/// Decode normalized sixteen-bit channels without quantizing high-depth input to eight bits.
pub(crate) fn pixel16(
    format: u32,
    x: usize,
    y: usize,
    color: (ColorEncoding, ColorRange),
    read: impl FnMut(usize, usize, usize, &mut [u8]) -> Result,
) -> Result<[u32; 3]> {
    channels(format, x, y, 65535, Some(color), read)
}

fn channels(
    format: u32,
    x: usize,
    y: usize,
    maximum: u64,
    color: Option<(ColorEncoding, ColorRange)>,
    mut read: impl FnMut(usize, usize, usize, &mut [u8]) -> Result,
) -> Result<[u32; 3]> {
    if let Some((hs, vs, planar, swap, depth)) = yuv(format) {
        let bytes = if depth == 8 { 1 } else { 2 };
        let mut luma = [0; 2];
        let mut chroma = [0; 4];
        read(0, x * bytes, y, &mut luma[..bytes])?;
        if planar {
            read(1, x / hs, y / vs, &mut chroma[..1])?;
            read(2, x / hs, y / vs, &mut chroma[1..2])?;
        } else {
            read(1, (x / hs) * bytes * 2, y / vs, &mut chroma[..bytes * 2])?;
        }
        if let Some((encoding, range)) = color {
            let sample = |b: &[u8]| -> i64 {
                if depth == 8 {
                    i64::from(b[0])
                } else {
                    i64::from(u16::from_le_bytes([b[0], b[1]]) >> (16 - depth))
                }
            };
            let unit = 1i64 << (depth - 8);
            let (y_offset, y_range, chroma_range) = match range {
                ColorRange::Limited => (16 * unit, 219 * unit, 224 * unit),
                ColorRange::Full => (0, (1 << depth) - 1, (1 << depth) - 1),
            };
            // Normalize code ranges with eight fractional bits, retaining high-depth
            // precision and the exact neutral chroma code at each input depth.
            let yy = (sample(&luma) - y_offset) * 65535 * 256 / y_range;
            let a = (sample(&chroma[..bytes]) - 128 * unit) * 65535 * 256 / chroma_range;
            let b = (sample(&chroma[bytes..]) - 128 * unit) * 65535 * 256 / chroma_range;
            let (u, v) = if swap { (b, a) } else { (a, b) };
            let [rv, gu, gv, bu]: [i64; 4] = match encoding {
                ColorEncoding::Bt601 => [6021544149, -1478054095, -3067191994, 7610682049],
                ColorEncoding::Bt709 => [6763714498, -804551626, -2010578443, 7969741314],
                ColorEncoding::Bt2020 => [6333358775, -706750298, -2453942994, 8080551471],
            };
            let channel = |value: i64| ((value + (1 << 39)) >> 40).clamp(0, 65535) as u32;
            let yy = yy << 32;
            return Ok([
                channel(yy + rv * v),
                channel(yy + gu * u + gv * v),
                channel(yy + bu * u),
            ]);
        }
        // Retain sub-byte precision for high-bit-depth input until RGB quantization.
        let sample = |b: &[u8]| -> i64 {
            if depth == 8 {
                i64::from(b[0]) * 256
            } else {
                i64::from(u16::from_le_bytes([b[0], b[1]]) & (u16::MAX << (16 - depth)))
            }
        };
        let yy = sample(&luma) - 16 * 256;
        let a = sample(&chroma[..bytes]) - 128 * 256;
        let b = sample(&chroma[bytes..]) - 128 * 256;
        let (u, v) = if swap { (b, a) } else { (a, b) };
        let channel = |v: i64| {
            let value = (v.max(0) * maximum as i64 + 255 * (1 << 23)) / (255 * (1 << 24));
            value.min(maximum as i64) as u32
        };
        return Ok([
            channel(76284 * yy + 104595 * v),
            channel(76284 * yy - 25624 * u - 53281 * v),
            channel(76284 * yy + 132251 * u),
        ]);
    }
    let bits = plane(format, 0)?.bits;
    let mut bytes = [0; 8];
    read(0, x * bits / 8, y, &mut bytes[..bits.div_ceil(8)])?;
    let word = u64::from_le_bytes(bytes);
    let scale = |v: u64, bits: u32| -> u32 {
        let max = (1 << bits) - 1;
        ((v * maximum + max / 2) / max) as u32
    };
    let (r, g, b) = match format {
        XRGB8888 | ARGB8888 | RGB888 => (
            scale(bytes[2] as u64, 8),
            scale(bytes[1] as u64, 8),
            scale(bytes[0] as u64, 8),
        ),
        XBGR8888 | ABGR8888 | BGR888 => (
            scale(bytes[0] as u64, 8),
            scale(bytes[1] as u64, 8),
            scale(bytes[2] as u64, 8),
        ),
        RGBA8888 => (
            scale(bytes[3] as u64, 8),
            scale(bytes[2] as u64, 8),
            scale(bytes[1] as u64, 8),
        ),
        BGRA8888 => (
            scale(bytes[1] as u64, 8),
            scale(bytes[2] as u64, 8),
            scale(bytes[3] as u64, 8),
        ),
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
    Ok([r, g, b])
}
