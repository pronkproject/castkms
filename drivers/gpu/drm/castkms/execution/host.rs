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

pub(crate) const MAX_WIDTH: u32 = 1920;
pub(crate) const MAX_HEIGHT: u32 = 1080;
pub(crate) const MAX_ALLOCATION_BYTES: usize = 16 * 1024 * 1024;

/// Validate native storage and full-frame sampling without granting source access.
///
/// An imported allocation is not qualified by successful PRIME import alone. The
/// built-in renderer supports only native CastKMS shmem and complete aligned rows.
pub(crate) fn check_framebuffer(image: &Framebuffer<Driver>, geometry: Geometry) -> Result {
    let width = image.width();
    let height = image.height();
    if width == 0 || height == 0 || width > MAX_WIDTH || height > MAX_HEIGHT {
        return Err(EINVAL);
    }
    if image.format() != fourcc::XRGB8888
        || image.plane_count() != 1
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
    let object = image.object_at(0)?;
    if object.imported_dma_buf().is_some() {
        return Err(EOPNOTSUPP);
    }
    let pitch = image.pitch(0)? as usize;
    let offset = image.offset(0)? as usize;
    if pitch % 4 != 0 || offset % 4 != 0 || pitch < width as usize * 4 {
        return Err(EINVAL);
    }
    let size = object.size();
    if size > MAX_ALLOCATION_BYTES {
        return Err(E2BIG);
    }
    let end = pitch
        .checked_mul(height as usize)
        .and_then(|bytes| bytes.checked_add(offset))
        .ok_or(EOVERFLOW)?;
    if end > size {
        return Err(EINVAL);
    }
    Ok(())
}
