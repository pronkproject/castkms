// SPDX-License-Identifier: GPL-2.0 OR MIT

//! DRM fourcc bindings.
//!
//! C header: [`include/uapi/drm/drm_fourcc.h`](srctree/include/uapi/drm/drm_fourcc.h)

/// Return a fourcc format code.
const fn fourcc_code(a: u8, b: u8, c: u8, d: u8) -> u32 {
    (a as u32) | (b as u32) << 8 | (c as u32) << 16 | (d as u32) << 24
}

const fn modifier_code(vendor: u8, value: u64) -> u64 {
    (vendor as u64) << 56 | (value & 0x00ff_ffff_ffff_ffff)
}

// TODO: We manually import this because we don't have a reasonable way of getting constants from
// function-like macros in bindgen yet.
/// Sentinel used when no explicit framebuffer modifier was supplied.
pub const FORMAT_MOD_INVALID: u64 = 0xffffffffffffff;
/// Linear framebuffer layout (`DRM_FORMAT_MOD_LINEAR`).
///
/// A driver that accepts only linear scanout has to say so through the plane's format-modifier
/// list, or userspace sees no `IN_FORMATS` property and has to guess what the plane will take.
pub const FORMAT_MOD_LINEAR: u64 = 0;

/// Intel X-tiled layout (`I915_FORMAT_MOD_X_TILED`).
pub const I915_FORMAT_MOD_X_TILED: u64 = modifier_code(0x01, 1);
/// Intel Y-tiled layout (`I915_FORMAT_MOD_Y_TILED`).
pub const I915_FORMAT_MOD_Y_TILED: u64 = modifier_code(0x01, 2);
/// Intel Yf-tiled layout (`I915_FORMAT_MOD_Yf_TILED`).
pub const I915_FORMAT_MOD_YF_TILED: u64 = modifier_code(0x01, 3);
/// Intel Tile 4 layout (`I915_FORMAT_MOD_4_TILED`).
pub const I915_FORMAT_MOD_4_TILED: u64 = modifier_code(0x01, 9);

/// 32 bpp RGB with unused alpha.
pub const XRGB8888: u32 = fourcc_code(b'X', b'R', b'2', b'4');

/// 32 bpp RGB with alpha.
pub const ARGB8888: u32 = fourcc_code(b'A', b'R', b'2', b'4');

/// 32 bpp BGR with unused alpha.
pub const XBGR8888: u32 = fourcc_code(b'X', b'B', b'2', b'4');

/// 32 bpp BGR with alpha.
pub const ABGR8888: u32 = fourcc_code(b'A', b'B', b'2', b'4');

/// Two-plane YUV 4:2:0 with an interleaved CbCr chroma plane.
pub const NV12: u32 = fourcc_code(b'N', b'V', b'1', b'2');

/// 32 bpp RGB with 10 bits per color component and two unused bits.
pub const XRGB2101010: u32 = fourcc_code(b'X', b'R', b'3', b'0');

/// 32 bpp RGB with 10 bits per color component and two alpha bits.
pub const ARGB2101010: u32 = fourcc_code(b'A', b'R', b'3', b'0');

/// 32 bpp BGR with 10 bits per color component and two unused bits.
pub const XBGR2101010: u32 = fourcc_code(b'X', b'B', b'3', b'0');

/// 32 bpp BGR with 10 bits per color component and two alpha bits.
pub const ABGR2101010: u32 = fourcc_code(b'A', b'B', b'3', b'0');

/// `[31:0]` B:G:R:A 8:8:8:8 little endian.
pub const BGRA8888: u32 = fourcc_code(b'B', b'A', b'2', b'4');

/// `[31:0]` R:G:B:A 8:8:8:8 little endian.
pub const RGBA8888: u32 = fourcc_code(b'R', b'A', b'2', b'4');

/// `[23:0]` R:G:B little endian.
pub const RGB888: u32 = fourcc_code(b'R', b'G', b'2', b'4');

/// `[23:0]` B:G:R little endian.
pub const BGR888: u32 = fourcc_code(b'B', b'G', b'2', b'4');

/// `[15:0]` x:R:G:B 4:4:4:4 little endian.
pub const XRGB4444: u32 = fourcc_code(b'X', b'R', b'1', b'2');
/// `[15:0]` x:B:G:R 4:4:4:4 little endian.
pub const XBGR4444: u32 = fourcc_code(b'X', b'B', b'1', b'2');
/// `[15:0]` R:G:B:x 4:4:4:4 little endian.
pub const RGBX4444: u32 = fourcc_code(b'R', b'X', b'1', b'2');
/// `[15:0]` B:G:R:x 4:4:4:4 little endian.
pub const BGRX4444: u32 = fourcc_code(b'B', b'X', b'1', b'2');
/// `[15:0]` A:R:G:B 4:4:4:4 little endian.
pub const ARGB4444: u32 = fourcc_code(b'A', b'R', b'1', b'2');
/// `[15:0]` A:B:G:R 4:4:4:4 little endian.
pub const ABGR4444: u32 = fourcc_code(b'A', b'B', b'1', b'2');
/// `[15:0]` R:G:B:A 4:4:4:4 little endian.
pub const RGBA4444: u32 = fourcc_code(b'R', b'A', b'1', b'2');
/// `[15:0]` B:G:R:A 4:4:4:4 little endian.
pub const BGRA4444: u32 = fourcc_code(b'B', b'A', b'1', b'2');

/// `[15:0]` x:R:G:B 1:5:5:5 little endian.
pub const XRGB1555: u32 = fourcc_code(b'X', b'R', b'1', b'5');
/// `[15:0]` x:B:G:R 1:5:5:5 little endian.
pub const XBGR1555: u32 = fourcc_code(b'X', b'B', b'1', b'5');
/// `[15:0]` R:G:B:x 5:5:5:1 little endian.
pub const RGBX5551: u32 = fourcc_code(b'R', b'X', b'1', b'5');
/// `[15:0]` B:G:R:x 5:5:5:1 little endian.
pub const BGRX5551: u32 = fourcc_code(b'B', b'X', b'1', b'5');
/// `[15:0]` A:R:G:B 1:5:5:5 little endian.
pub const ARGB1555: u32 = fourcc_code(b'A', b'R', b'1', b'5');
/// `[15:0]` A:B:G:R 1:5:5:5 little endian.
pub const ABGR1555: u32 = fourcc_code(b'A', b'B', b'1', b'5');
/// `[15:0]` R:G:B:A 5:5:5:1 little endian.
pub const RGBA5551: u32 = fourcc_code(b'R', b'A', b'1', b'5');
/// `[15:0]` B:G:R:A 5:5:5:1 little endian.
pub const BGRA5551: u32 = fourcc_code(b'B', b'A', b'1', b'5');

/// `[31:0]` R:G:B:x 8:8:8:8 little endian.
pub const RGBX8888: u32 = fourcc_code(b'R', b'X', b'2', b'4');
/// `[31:0]` B:G:R:x 8:8:8:8 little endian.
pub const BGRX8888: u32 = fourcc_code(b'B', b'X', b'2', b'4');

/// `[31:0]` R:G:B:x 10:10:10:2 little endian.
pub const RGBX1010102: u32 = fourcc_code(b'R', b'X', b'3', b'0');
/// `[31:0]` B:G:R:x 10:10:10:2 little endian.
pub const BGRX1010102: u32 = fourcc_code(b'B', b'X', b'3', b'0');
/// `[31:0]` R:G:B:A 10:10:10:2 little endian.
pub const RGBA1010102: u32 = fourcc_code(b'R', b'A', b'3', b'0');
/// `[31:0]` B:G:R:A 10:10:10:2 little endian.
pub const BGRA1010102: u32 = fourcc_code(b'B', b'A', b'3', b'0');

/// `[63:0]` x:R:G:B 16:16:16:16 little endian.
pub const XRGB16161616: u32 = fourcc_code(b'X', b'R', b'4', b'8');

/// `[63:0]` x:B:G:R 16:16:16:16 little endian.
pub const XBGR16161616: u32 = fourcc_code(b'X', b'B', b'4', b'8');

/// `[63:0]` A:R:G:B 16:16:16:16 little endian.
pub const ARGB16161616: u32 = fourcc_code(b'A', b'R', b'4', b'8');

/// `[63:0]` A:B:G:R 16:16:16:16 little endian.
pub const ABGR16161616: u32 = fourcc_code(b'A', b'B', b'4', b'8');

/// 64 bpp RGB with binary16 components and unused alpha.
pub const XRGB16161616F: u32 = fourcc_code(b'X', b'R', b'4', b'H');
/// 64 bpp BGR with binary16 components and unused alpha.
pub const XBGR16161616F: u32 = fourcc_code(b'X', b'B', b'4', b'H');
/// 64 bpp RGBA with binary16 components.
pub const ARGB16161616F: u32 = fourcc_code(b'A', b'R', b'4', b'H');
/// 64 bpp BGRA with binary16 components.
pub const ABGR16161616F: u32 = fourcc_code(b'A', b'B', b'4', b'H');

/// `[15:0]` R:G:B 5:6:5 little endian.
pub const RGB565: u32 = fourcc_code(b'R', b'G', b'1', b'6');

/// `[15:0]` B:G:R 5:6:5 little endian.
pub const BGR565: u32 = fourcc_code(b'B', b'G', b'1', b'6');

/// 2x1 subsampled Cr:Cb plane.
pub const NV16: u32 = fourcc_code(b'N', b'V', b'1', b'6');

/// non-subsampled Cr:Cb plane.
pub const NV24: u32 = fourcc_code(b'N', b'V', b'2', b'4');

/// 2x2 subsampled Cb:Cr plane.
pub const NV21: u32 = fourcc_code(b'N', b'V', b'2', b'1');

/// 2x1 subsampled Cb:Cr plane.
pub const NV61: u32 = fourcc_code(b'N', b'V', b'6', b'1');

/// non-subsampled Cb:Cr plane.
pub const NV42: u32 = fourcc_code(b'N', b'V', b'4', b'2');

/// 2x2 subsampled Cb (1) and Cr (2) planes.
pub const YUV420: u32 = fourcc_code(b'Y', b'U', b'1', b'2');

/// 2x1 subsampled Cb (1) and Cr (2) planes.
pub const YUV422: u32 = fourcc_code(b'Y', b'U', b'1', b'6');

/// non-subsampled Cb (1) and Cr (2) planes.
pub const YUV444: u32 = fourcc_code(b'Y', b'U', b'2', b'4');

/// 2x2 subsampled Cr (1) and Cb (2) planes.
pub const YVU420: u32 = fourcc_code(b'Y', b'V', b'1', b'2');

/// 2x1 subsampled Cr (1) and Cb (2) planes.
pub const YVU422: u32 = fourcc_code(b'Y', b'V', b'1', b'6');

/// non-subsampled Cr (1) and Cb (2) planes.
pub const YVU444: u32 = fourcc_code(b'Y', b'V', b'2', b'4');

/// 2x2 subsampled Cr:Cb plane 10 bits per channel.
pub const P010: u32 = fourcc_code(b'P', b'0', b'1', b'0');

/// 2x2 subsampled Cr:Cb plane 12 bits per channel.
pub const P012: u32 = fourcc_code(b'P', b'0', b'1', b'2');

/// 2x2 subsampled Cr:Cb plane 16 bits per channel.
pub const P016: u32 = fourcc_code(b'P', b'0', b'1', b'6');

/// `[7:0]` R0:R1:R2:R3:R4:R5:R6:R7 1:1:1:1:1:1:1:1 eight pixels/byte.
pub const R1: u32 = fourcc_code(b'R', b'1', b' ', b' ');

/// `[7:0]` R0:R1:R2:R3 2:2:2:2 four pixels/byte.
pub const R2: u32 = fourcc_code(b'R', b'2', b' ', b' ');

/// `[7:0]` R0:R1 4:4 two pixels/byte.
pub const R4: u32 = fourcc_code(b'R', b'4', b' ', b' ');

/// `[7:0]` R.
pub const R8: u32 = fourcc_code(b'R', b'8', b' ', b' ');
