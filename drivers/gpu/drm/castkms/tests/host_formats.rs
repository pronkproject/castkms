// SPDX-License-Identifier: GPL-2.0-only

//! Pixel decoding through real framebuffer storage, atomic publication and composition.

use super::*;
use crate::{
    formats,
    host_compositor::{compose, layout::Layout, pool::Pool},
};
use kernel::io::Io;

fn white_image(format: u32, shared: bool) -> Result {
    let fixture = Fixture::new()?;
    let count = formats::plane_count(format);
    let mut objects = KVec::new();
    for _ in 0..if shared { 1 } else { count } {
        objects.push(
            shmem::Object::<gem::Object>::new(
                fixture.drm.device(),
                16384,
                Default::default(),
                Default::default(),
            )?,
            GFP_KERNEL,
        )?;
    }
    let mut planes = KVec::new();
    for index in 0..count {
        let object = &objects[if shared { 0 } else { index }];
        let layout = formats::plane(format, index)?;
        let pitch = layout.row_bytes(17) + 3;
        let offset = index * 4096 + 1;
        let map = object.vmap::<0>()?;
        for y in 0..layout.rows(3) {
            for x in 0..layout.row_bytes(17) {
                let value = if count == 1 {
                    255
                } else if matches!(
                    format,
                    drm::fourcc::P010 | drm::fourcc::P012 | drm::fourcc::P016
                ) {
                    if x % 2 == 0 {
                        0
                    } else if index == 0 {
                        235
                    } else {
                        128
                    }
                } else if index == 0 {
                    235
                } else {
                    128
                };
                map.try_write8(value, offset + y * pitch + x)?;
            }
        }
        planes.push(
            FramebufferPlane {
                object: &**object,
                pitch: pitch as u32,
                offset: offset as u32,
            },
            GFP_KERNEL,
        )?;
    }
    let fb = fixture.drm.framebuffer(
        &FramebufferLayout {
            width: 17,
            height: 3,
            format,
            modifier: Some(drm::fourcc::FORMAT_MOD_LINEAR),
            interlaced: false,
            planes: &planes,
        },
        provenance::Provenance::from_snapshot(None),
    )?;
    let mode = DisplayMode::from_timings(ModeTimings {
        clock_khz: 15,
        hdisplay: 17,
        hsync_start: 25,
        hsync_end: 33,
        htotal: 41,
        vdisplay: 3,
        vsync_start: 4,
        vsync_end: 5,
        vtotal: 6,
        flags: ModeFlags::NHSYNC | ModeFlags::NVSYNC,
    })?;
    let scanout = CrtcScanout {
        mode: &mode,
        framebuffer: &fb,
        connectors: &[fixture.drm.connector()?],
        position: (0, 0),
    };
    fixture.drm.update(|mut transaction| {
        transaction
            .as_mut()
            .set_crtc_config(fixture.drm.crtc()?, Some(&scanout))
    })?;
    let pool = Pool::new(
        fixture.drm.device(),
        &fixture.host_budget,
        Layout::new(17, 3)?,
    )?;
    let image = compose::current(&fixture.drm.device().output, &pool)?.ok_or(EINVAL)?;
    let mut row = [0; 68];
    for y in 0..3 {
        image.read_row(y, &mut row)?;
        for pixel in row.chunks_exact(4) {
            check(pixel[..3] == [255; 3])?;
        }
    }
    Ok(())
}

#[kunit_tests(rust_castkms_host_formats)]
mod cases {
    use super::*;

    #[test]
    fn packed_channel_order_decodes_red() -> Result {
        use drm::fourcc::*;
        for (format, bytes) in [
            (ARGB8888, 0x80ff0000u64),
            (ABGR8888, 0x800000ff),
            (RGBA8888, 0xff000080),
            (BGRA8888, 0x0000ff80),
            (RGB888, 0xff0000),
            (BGR888, 0xff),
            (RGB565, 0xf800),
            (BGR565, 0x001f),
            (XRGB2101010, 0x3ff00000),
            (XBGR2101010, 0x3ff),
            (ARGB2101010, 0xbff00000),
            (ABGR2101010, 0x800003ff),
            (XRGB16161616, 0x0000ffff00000000),
            (ARGB16161616, 0x8000ffff00000000),
            (XBGR16161616, 0x000000000000ffff),
            (ABGR16161616, 0x800000000000ffff),
        ] {
            let pixel = formats::pixel(format, 0, 0, |plane, x, y, out| {
                check(plane == 0 && x == 0 && y == 0)?;
                out.copy_from_slice(&bytes.to_le_bytes()[..out.len()]);
                Ok(())
            })?;
            check(pixel == 0xff0000)?;
        }
        Ok(())
    }

    #[test]
    fn high_depth_yuv_ignores_unused_low_bits() -> Result {
        for (format, padding) in [(drm::fourcc::P010, 63u16), (drm::fourcc::P012, 15u16)] {
            let pixel = formats::pixel(format, 0, 0, |plane, _, _, out| {
                for word in out.chunks_exact_mut(2) {
                    word.copy_from_slice(
                        &((if plane == 0 { 16u16 } else { 128u16 }) * 256 | padding).to_le_bytes(),
                    );
                }
                Ok(())
            })?;
            check(pixel == 0)?;
        }
        Ok(())
    }

    #[test]
    fn every_format_decodes_padded_odd_width_storage() -> Result {
        for &format in formats::FORMATS {
            white_image(format, true)?;
        }
        Ok(())
    }

    #[test]
    fn chroma_planes_may_use_independent_objects() -> Result {
        for &format in formats::FORMATS {
            if formats::plane_count(format) > 1 {
                white_image(format, false)?;
            }
        }
        Ok(())
    }

    #[test]
    fn monochrome_pixels_are_most_significant_bit_first() -> Result {
        for (format, values) in [
            (drm::fourcc::R1, [0xffffff, 0]),
            (drm::fourcc::R2, [0xaaaaaa, 0x555555]),
            (drm::fourcc::R4, [0x999999, 0x666666]),
        ] {
            for (x, expected) in values.into_iter().enumerate() {
                let pixel = formats::pixel(format, x, 0, |_, _, _, out| {
                    out[0] = 0x96;
                    Ok(())
                })?;
                check(pixel == expected)?;
            }
        }
        Ok(())
    }

    #[test]
    fn yuv_chroma_order_selects_red() -> Result {
        for (format, chroma) in [
            (drm::fourcc::NV12, [90, 240]),
            (drm::fourcc::NV21, [240, 90]),
        ] {
            let pixel = formats::pixel(format, 3, 3, |plane, x, y, out| {
                if plane == 0 {
                    check(x == 3 && y == 3)?;
                    out[0] = 81;
                } else {
                    check(x == 2 && y == 1)?;
                    out.copy_from_slice(&chroma);
                }
                Ok(())
            })?;
            check(pixel == 0xfe0000)?;
        }
        Ok(())
    }
}
