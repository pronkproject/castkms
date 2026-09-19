// SPDX-License-Identifier: GPL-2.0-only

//! Destination validation retains allocation identity without mapping or reading sources.

use super::*;
use crate::{
    capture::destination::Image,
    host_compositor::layout::Layout, //
};
use kernel::drm::fourcc;

#[kunit_tests(rust_castkms_capture_destinations)]
mod cases {
    use super::*;

    #[test]
    fn padded_rows_retain_the_exact_buffer_and_visible_layout() -> Result {
        with_exporter(|fixture| {
            let buffer = fixture.drm.export_dumb(64, 64, 32)?;
            let layout = Layout::new(13, 11)?;
            let image = Image::new(
                buffer.clone(),
                layout,
                fourcc::XRGB8888,
                fourcc::FORMAT_MOD_LINEAR,
                64,
                128,
            )?;
            check(core::ptr::eq(image.buffer(), &*buffer))?;
            check(image.dimensions() == [13, 11])?;
            check(image.pitch() == 64)?;
            check(image.offset() == 128)?;
            drop(buffer);
            check(image.buffer().size() == 16384)
        })
    }

    #[test]
    fn packed_formats_accept_exact_modifiers_and_reject_invalid_metadata() -> Result {
        with_exporter(|fixture| {
            let buffer = fixture.drm.export_dumb(64, 64, 32)?;
            let layout = Layout::new(64, 64)?;
            for (format, modifier) in [
                (fourcc::XRGB8888, fourcc::FORMAT_MOD_LINEAR),
                (fourcc::ARGB8888, fourcc::FORMAT_MOD_LINEAR),
                (fourcc::XBGR8888, 9),
                (fourcc::ABGR8888, 9),
            ] {
                check(Image::new(buffer.clone(), layout, format, modifier, 256, 0).is_ok())?;
            }
            check(matches!(Image::new(
                buffer.clone(), layout, fourcc::RGB565,
                fourcc::FORMAT_MOD_LINEAR, 256, 0,
            ), Err(EOPNOTSUPP)))?;
            check(matches!(Image::new(
                buffer, layout, fourcc::ARGB8888,
                fourcc::FORMAT_MOD_INVALID, 256, 0,
            ), Err(EOPNOTSUPP)))?;
            Ok(())
        })
    }

    #[test]
    fn tiled_metadata_does_not_claim_linear_row_storage() -> Result {
        with_exporter(|fixture| {
            let buffer = fixture.drm.export_dumb(64, 64, 32)?;
            let layout = Layout::new(64, 64)?;
            let image = Image::new(buffer.clone(), layout, fourcc::ARGB8888, 9, 512, 0)?;
            check(image.pitch() == 512)?;
            check(matches!(Image::new(
                buffer, layout, fourcc::ARGB8888, 9, 512, 16384,
            ), Err(EINVAL)))
        })
    }

    #[test]
    fn every_row_including_padding_must_fit_the_allocation() -> Result {
        with_exporter(|fixture| {
            let buffer = fixture.drm.export_dumb(64, 64, 32)?;
            let layout = Layout::new(64, 64)?;
            for (pitch, offset) in [(252, 0), (257, 0), (256, 1), (256, 4), (260, 0)] {
                check(matches!(
                    Image::new(
                        buffer.clone(),
                        layout,
                        fourcc::XRGB8888,
                        fourcc::FORMAT_MOD_LINEAR,
                        pitch,
                        offset
                    ),
                    Err(EINVAL)
                ))?;
            }
            check(matches!(
                Image::new(
                    buffer.clone(),
                    layout,
                    fourcc::XRGB8888,
                    fourcc::FORMAT_MOD_LINEAR,
                    usize::MAX - 3,
                    0
                ),
                Err(EOVERFLOW)
            ))?;
            check(matches!(
                Image::new(
                    buffer.clone(),
                    layout,
                    fourcc::XRGB8888,
                    fourcc::FORMAT_MOD_LINEAR,
                    256,
                    usize::MAX - 3
                ),
                Err(EOVERFLOW)
            ))?;
            let _image = Image::new(
                buffer,
                layout,
                fourcc::XRGB8888,
                fourcc::FORMAT_MOD_LINEAR,
                256,
                0,
            )?;
            Ok(())
        })
    }
}
