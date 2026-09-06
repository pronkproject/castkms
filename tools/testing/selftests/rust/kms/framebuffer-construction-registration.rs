// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0308\]
#![no_std]

use kernel::{
    drm::{
        kms::{
            framebuffer::{Framebuffer, FramebufferLayout, FramebufferRef},
            KmsDriver,
        },
        Device, Registered,
    },
    prelude::*,
};

pub fn registered<T: KmsDriver>(
    dev: &Device<T, Registered>,
    layout: &FramebufferLayout<'_, T>,
) -> Result<FramebufferRef<T>> {
    Framebuffer::from_objects(dev, layout)
}

#[cfg(negative)]
pub fn unguarded<T: KmsDriver>(
    dev: &Device<T>,
    layout: &FramebufferLayout<'_, T>,
) -> Result<FramebufferRef<T>> {
    Framebuffer::from_objects(dev, layout)
}
