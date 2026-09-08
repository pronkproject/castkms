// SPDX-License-Identifier: GPL-2.0-only

//! Owned display descriptions, without pixel-read permission or completion guarantees.

use super::Driver;
use kernel::{
    drm::kms::framebuffer::FramebufferRef,
    prelude::*, //
};

/// Requested source coordinates are retained in DRM's unsigned 16.16 representation.
#[derive(Clone, Copy)]
pub(super) struct Geometry {
    pub(super) source: [u32; 4],
    pub(super) destination: [u32; 2],
}

/// The framebuffer reference preserves storage lifetime, not the contents of that storage.
pub(super) struct Scene {
    _framebuffer: FramebufferRef<Driver>,
    _source: [u32; 4],
    _destination: [u32; 2],
}

impl Scene {
    pub(super) fn new(framebuffer: FramebufferRef<Driver>, geometry: Geometry) -> Self {
        Self {
            _framebuffer: framebuffer,
            _source: geometry.source,
            _destination: geometry.destination,
        }
    }
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
mod tests;
