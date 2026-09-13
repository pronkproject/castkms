// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0599\]
#![no_std]

use kernel::drm::capture::{
    Authority,
    Policy,
    Revocation, //
};

pub fn revoke<P: Policy>(authority: &Authority<P>) {
    authority.revocation().revoke();
}

#[cfg(negative)]
pub fn admit_without_provider(revocation: &Revocation) {
    let _ = revocation.begin();
}
