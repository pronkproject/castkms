// SPDX-License-Identifier: GPL-2.0 OR MIT
// error-pattern: cannot move out of `registered` because it is borrowed
#![no_std]

use kernel::{
    drm::{
        device::{
            Device,
            Ioctl, //
        },
        kms::{
            testing::RegisteredMasterFile,
            KmsDriver, //
        }, //
    },
    prelude::*, //
};

pub fn use_file<T: KmsDriver>(device: &Device<T, Ioctl>) -> Result {
    let registered = device.registration_guard().ok_or(ENODEV)?;
    let file = RegisteredMasterFile::new(&registered)?;
    #[cfg(negative)]
    drop(registered);
    core::hint::black_box(file.file());
    Ok(())
}
