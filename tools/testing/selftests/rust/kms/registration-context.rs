// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0599\]
#![no_std]

use kernel::drm::{Device, Driver, Registration, RegistrationGuard};

pub fn completed<D: Driver>(registration: &Registration<'_, D>) {
    let _: Option<RegistrationGuard<'_, D>> = registration.registration_guard();
}

#[cfg(negative)]
pub fn not_registered<D: Driver>(device: &Device<D>) {
    let _ = device.registration_guard();
}
