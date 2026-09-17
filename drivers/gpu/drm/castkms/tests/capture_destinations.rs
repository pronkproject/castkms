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
    fn unsupported_formats_do_not_qualify_a_destination() -> Result {
        with_exporter(|fixture| {
            let buffer = fixture.drm.export_dumb(64, 64, 32)?;
            let layout = Layout::new(64, 64)?;
            check(matches!(
                Image::new(
                    buffer.clone(),
                    layout,
                    fourcc::ARGB8888,
                    fourcc::FORMAT_MOD_LINEAR,
                    256,
                    0
                ),
                Err(EOPNOTSUPP)
            ))?;
            check(matches!(
                Image::new(buffer.clone(), layout, fourcc::XRGB8888, 1, 256, 0),
                Err(EOPNOTSUPP)
            ))?;
            Ok(())
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
