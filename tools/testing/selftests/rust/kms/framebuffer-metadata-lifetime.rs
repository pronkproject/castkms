// SPDX-License-Identifier: GPL-2.0
// error-pattern: lifetime may not live long enough
#![no_std]

use kernel::drm::kms::{framebuffer::Framebuffer, KmsDriver};

pub fn borrowed<D: KmsDriver>(fb: &Framebuffer<D>) -> Option<&D::FramebufferData> {
    fb.data()
}

#[cfg(negative)]
pub fn escaped<D: KmsDriver>(fb: &Framebuffer<D>) -> Option<&'static D::FramebufferData> {
    fb.data()
}
