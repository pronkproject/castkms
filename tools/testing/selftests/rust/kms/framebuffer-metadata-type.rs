// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0308\]
#![no_std]

use kernel::drm::{
    device::{Device, Registered},
    kms::{
        framebuffer::{Framebuffer, FramebufferLayout, FramebufferRef},
        KmsDriver,
    },
};
use kernel::error::Result;

pub fn nominated<D: KmsDriver>(
    dev: &Device<D, Registered>,
    layout: &FramebufferLayout<'_, D>,
    data: D::FramebufferData,
) -> Result<FramebufferRef<D>> {
    Framebuffer::from_objects_with_data(dev, layout, data)
}

#[cfg(negative)]
pub fn arbitrary<D: KmsDriver, M>(
    dev: &Device<D, Registered>,
    layout: &FramebufferLayout<'_, D>,
    data: M,
) -> Result<FramebufferRef<D>> {
    Framebuffer::from_objects_with_data(dev, layout, data)
}
