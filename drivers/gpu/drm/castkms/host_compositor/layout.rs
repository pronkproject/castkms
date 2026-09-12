// SPDX-License-Identifier: GPL-2.0-only

//! Validated geometry for private packed XRGB8888 images.

use kernel::{
    page::page_align,
    prelude::*, //
};

/// Allocation geometry checked without allocating storage or reserving budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Layout {
    width: u32,
    height: u32,
    pitch: usize,
    size: usize,
}

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Layout {
    pub(crate) fn new(width: u32, height: u32) -> Result<Self> {
        if width == 0 || height == 0 || width > 1920 || height > 1080 {
            return Err(EINVAL);
        }
        let pitch = (width as usize).checked_mul(4).ok_or(EOVERFLOW)?;
        let bytes = pitch.checked_mul(height as usize).ok_or(EOVERFLOW)?;
        let size = page_align(bytes).ok_or(EOVERFLOW)?;
        if size > 8 * 1024 * 1024 {
            return Err(E2BIG);
        }
        Ok(Self {
            width,
            height,
            pitch,
            size,
        })
    }

    pub(crate) fn dimensions(self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub(crate) fn pitch(self) -> usize {
        self.pitch
    }

    /// Complete allocation size, including the final page's padding.
    pub(crate) fn size(self) -> usize {
        self.size
    }
}
