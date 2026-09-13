// SPDX-License-Identifier: GPL-2.0-only

//! Checked storage and sampling geometry, without pixel-read authority.

use crate::{
    execution::host,
    scene::Geometry,
    Driver, //
};
use kernel::{
    drm::kms::framebuffer::{
        Framebuffer as KmsFramebuffer,
        FramebufferRef, //
    },
    prelude::*, //
};

/// A retained native framebuffer satisfying the host compositor's storage limits.
///
/// Construction checks storage and sampling only. Scene color policy, capture authority and
/// source-read accounting remain separate. Mapping storage does not authorize pixel access.
pub(crate) struct Framebuffer {
    image: FramebufferRef<Driver>,
}

impl Framebuffer {
    pub(crate) fn new(image: &KmsFramebuffer<Driver>, geometry: Geometry) -> Result<Self> {
        host::check_framebuffer(image, geometry)?;
        Ok(Self {
            image: image.to_owned_ref(),
        })
    }

    pub(crate) fn dimensions(&self) -> (u32, u32) {
        (self.image.width(), self.image.height())
    }

    /// Prepare an owned native mapping without reading pixels or claiming the source.
    pub(super) fn prepare_mapping(
        &self,
    ) -> Result<kernel::drm::kms::framebuffer::FramebufferVMapOwned<crate::gem::Object>> {
        self.image.owned_vmap()
    }
}
