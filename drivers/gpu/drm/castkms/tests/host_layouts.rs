// SPDX-License-Identifier: GPL-2.0-only

use super::*;
use crate::host_compositor::{
    compose,
    layout::Layout,
    pool::Pool, //
};
use kernel::{
    drm::preparation::Source,
    io::Io,
    page::page_align,
    sync::aref::ARef, //
};

fn pattern(column: usize, row: u32) -> u8 {
    ((column as u32 * 17) ^ (row * 29) ^ 0x59) as u8
}

fn check_layout(width: u16, height: u16, padding: u32, offset: u32) -> Result {
    let layout = Layout::new(u32::from(width), u32::from(height))?;
    let fixture = Fixture::new()?;
    let packed = usize::from(width) * 4;
    let pitch = u32::from(width) * 4 + padding;
    let size = page_align(offset as usize + pitch as usize * usize::from(height))
        .ok_or(EOVERFLOW)?;
    let object = shmem::Object::<gem::Object>::new(
        fixture.drm.device(),
        size,
        Default::default(),
        Default::default(),
    )?;
    let fb = fixture.drm.framebuffer(
        &FramebufferLayout {
            width: u32::from(width),
            height: u32::from(height),
            format: drm::fourcc::XRGB8888,
            modifier: Some(drm::fourcc::FORMAT_MOD_LINEAR),
            interlaced: false,
            planes: &[FramebufferPlane {
                object: &object,
                pitch,
                offset,
            }],
        },
        provenance::Provenance::from_snapshot(None),
    )?;
    let mut row = KVVec::new();
    row.resize(pitch as usize, 0xde, GFP_KERNEL)?;
    {
        let mapping = object.vmap::<0>()?;
        for y in 0..u32::from(height) {
            for (x, pixel) in row[..packed].iter_mut().enumerate() {
                *pixel = pattern(x, y);
            }
            let start = offset as usize + y as usize * pitch as usize;
            for (x, byte) in row.iter().enumerate() {
                mapping.try_write8(*byte, start + x)?;
            }
        }
    }

    let mode = DisplayMode::from_timings(ModeTimings {
        clock_khz: i32::try_from(
            ((u32::from(width) + 24) * (u32::from(height) + 3) * 60).div_ceil(1000),
        )
        .map_err(|_| EOVERFLOW)?,
        hdisplay: width,
        hsync_start: width + 8,
        hsync_end: width + 16,
        htotal: width + 24,
        vdisplay: height,
        vsync_start: height + 1,
        vsync_end: height + 2,
        vtotal: height + 3,
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
    let pool = Pool::new(fixture.drm.device(), &fixture.host_budget, layout)?;
    let output = &fixture.drm.device().output;
    let source: ARef<Source> = output
        .inspect_accepted(|accepted| accepted.map(|(source, _)| source.into()))
        .ok_or(EINVAL)?;
    let image = compose::current(output, &pool)?.ok_or(EINVAL)?;
    if image.layout() != layout || source.hold_admission()?.prepared()?.is_none() {
        return Err(EINVAL);
    }

    // Only the private image should retain the pattern after source reuse.
    row.fill(0);
    {
        let mapping = object.vmap::<0>()?;
        for y in 0..usize::from(height) {
            let start = offset as usize + y * pitch as usize;
            for (x, byte) in row.iter().enumerate() {
                mapping.try_write8(*byte, start + x)?;
            }
        }
    }
    fixture.state.close();
    if source.prepared()?.is_none() {
        return Err(EBUSY);
    }
    let mut pixels = KVVec::new();
    pixels.resize(packed * usize::from(height), 0xa7, GFP_KERNEL)?;
    image.copy_pixels(&mut pixels)?;
    for (y, row) in pixels.chunks_exact(packed).enumerate() {
        if !row.iter().enumerate().all(|(x, pixel)| *pixel == pattern(x, y as u32)) {
            return Err(EIO);
        }
    }
    Ok(())
}

#[kunit_tests(rust_castkms_host_layouts)]
mod cases {
    use super::*;

    #[test]
    fn one_pixel_image() -> Result {
        check_layout(1, 1, 0, 0)
    }

    #[test]
    fn odd_width_with_padded_rows() -> Result {
        check_layout(3, 2, 4, 4)
    }

    #[test]
    fn offset_crossing_a_page_boundary() -> Result {
        check_layout(63, 17, 12, 4092)
    }

    #[test]
    fn vga_with_padding_and_offset() -> Result {
        check_layout(640, 480, 16, 64)
    }

    #[test]
    fn nearly_full_hd_with_an_odd_width() -> Result {
        check_layout(1919, 1079, 4, 4096)
    }

    #[test]
    fn full_hd_packed() -> Result {
        check_layout(1920, 1080, 0, 0)
    }

    #[test]
    fn full_hd_with_padding_and_offset() -> Result {
        check_layout(1920, 1080, 16, 4)
    }
}
