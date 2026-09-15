// SPDX-License-Identifier: GPL-2.0-only

//! Eligibility for the built-in renderer, without mapping or retaining pixels.

use crate::{
    scene::Geometry,
    Driver, //
};
use kernel::{
    drm::{
        fourcc,
        gem::BaseObject,
        kms::framebuffer::Framebuffer, //
    },
    prelude::*, //
};

pub(crate) const MAX_WIDTH: u32 = 8192;
pub(crate) const MAX_HEIGHT: u32 = 8192;
pub(crate) const MAX_ALLOCATION_BYTES: usize = 512 * 1024 * 1024;

pub(crate) use crate::formats::FORMATS;

/// Validate linear storage and full-frame sampling without granting source access.
pub(crate) fn check_framebuffer(image: &Framebuffer<Driver>, geometry: Geometry) -> Result {
    let width = image.width();
    let height = image.height();
    if width == 0 || height == 0 || width > MAX_WIDTH || height > MAX_HEIGHT {
        return Err(EINVAL);
    }
    if !FORMATS.contains(&image.format())
        || image.plane_count() != crate::formats::plane_count(image.format())
        || image.is_interlaced()
        || image
            .modifier()
            .is_some_and(|modifier| modifier != fourcc::FORMAT_MOD_LINEAR)
        || geometry.source != [0, 0, width << 16, height << 16]
        || geometry.destination != [width, height]
        || geometry.output != [width, height]
    {
        return Err(EINVAL);
    }
    for index in 0..image.plane_count() {
        let object = image.object_at(index)?;
        if object.imported_dma_buf().is_some() {
            return Err(EOPNOTSUPP);
        }
        let layout = crate::formats::plane(image.format(), index)?;
        let pitch = image.pitch(index)? as usize;
        let offset = image.offset(index)? as usize;
        if pitch < layout.row_bytes(width) {
            return Err(EINVAL);
        }
        if object.size() > MAX_ALLOCATION_BYTES {
            return Err(E2BIG);
        }
        let end = pitch
            .checked_mul(layout.rows(height) - 1)
            .and_then(|bytes| bytes.checked_add(layout.row_bytes(width)))
            .and_then(|bytes| bytes.checked_add(offset))
            .ok_or(EOVERFLOW)?;
        if end > object.size() {
            return Err(EINVAL);
        }
    }
    Ok(())
}
