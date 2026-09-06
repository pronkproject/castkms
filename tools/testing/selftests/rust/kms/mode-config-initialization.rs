// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0599\]
#![no_std]

use kernel::drm::{
    kms::{KmsDriver, UnregisteredKmsDevice},
    Device, Registered, UnregisteredDevice,
};

pub fn setup<D: KmsDriver>(dev: &UnregisteredKmsDevice<'_, D>) {
    drop(dev.mode_config_lock());
}

pub fn registered<D: KmsDriver>(dev: &Device<D, Registered>) {
    drop(dev.mode_config_lock());
}

#[cfg(negative)]
pub fn allocated<D: KmsDriver>(dev: &UnregisteredDevice<D>) {
    drop(dev.mode_config_lock());
}
