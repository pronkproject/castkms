// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0599\]
#![no_std]

use kernel::drm::{kms::KmsDriver, Device, Registered, UnregisteredDevice};

pub fn registered<D: KmsDriver>(dev: &Device<D, Registered>) {
    dev.hotplug_event();
}

#[cfg(negative)]
pub fn allocated<D: KmsDriver>(dev: &UnregisteredDevice<D>) {
    dev.hotplug_event();
}
