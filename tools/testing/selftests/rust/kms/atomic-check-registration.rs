// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0599\]
#![no_std]

use kernel::drm::{device::Registered, kms::KmsDriver, Device};

pub fn initialized<D: KmsDriver>(dev: &Device<D, Registered>) {
    let _ = dev.check_atomic_update(|_| Ok(()));
}

#[cfg(negative)]
pub fn too_early<D: KmsDriver>(dev: &Device<D>) {
    let _ = dev.check_atomic_update(|_| Ok(()));
}
