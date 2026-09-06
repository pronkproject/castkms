// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0277\]
#![no_std]

use kernel::drm::kms::{
    framebuffer::{Framebuffer, FramebufferRef},
    KmsDriver,
};

pub fn owned<D: KmsDriver>(fb: &Framebuffer<D>) -> FramebufferRef<D> {
    fb.to_owned_ref()
}

#[cfg(negative)]
pub fn without_device<D: KmsDriver>(
    fb: &Framebuffer<D>,
) -> kernel::sync::aref::ARef<Framebuffer<D>> {
    fb.into()
}
