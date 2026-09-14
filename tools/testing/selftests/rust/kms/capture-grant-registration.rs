// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0599\]
#![no_std]

use kernel::drm::{
    capture::Target,
    device::Registered,
    kms::KmsDriver,
    Device,
    File, //
};

pub fn initialized<D: KmsDriver>(dev: &Device<D, Registered>, file: &File<D::File>) {
    let _ = dev.create_capture_grant(file, Target::new(1, 2).unwrap());
}

#[cfg(negative)]
pub fn too_early<D: KmsDriver>(dev: &Device<D>, file: &File<D::File>) {
    let _ = dev.create_capture_grant(file, Target::new(1, 2).unwrap());
}
