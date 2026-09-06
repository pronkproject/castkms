// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0599\]
#![no_std]

use kernel::drm::{
    kms::{KmsDriver, UnregisteredKmsDevice},
    Device, Registered,
};

pub fn setup<D: KmsDriver>(dev: &UnregisteredKmsDevice<'_, D>) -> u32 {
    dev.num_crtcs()
}

pub fn registered<D: KmsDriver>(dev: &Device<D, Registered>) -> u32 {
    dev.num_crtcs()
}

#[cfg(negative)]
pub fn escaped_setup_device<D: KmsDriver>(dev: &Device<D>) -> u32 {
    // This reference may have escaped to another thread during object creation.
    dev.num_crtcs()
}
