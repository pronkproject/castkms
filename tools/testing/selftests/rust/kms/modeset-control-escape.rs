// SPDX-License-Identifier: GPL-2.0 OR MIT
// error-pattern: lifetime may not live long enough
#![no_std]

use kernel::{
    drm::{
        device::{
            Device,
            Registered, //
        },
        kms::KmsDriver, //
    },
    prelude::*, //
};

pub fn control<D: KmsDriver>(device: &Device<D, Registered>) -> Result {
    #[cfg(negative)]
    let _escaped = device.with_modeset_locks(|state| state)?;
    #[cfg(not(negative))]
    device.with_modeset_locks(|_| ())?;
    Ok(())
}
