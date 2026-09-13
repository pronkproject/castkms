// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0308\]
#![no_std]

use kernel::{
    drm::capture::{
        Authority,
        Policy,
        Revocation, //
    },
    sync::aref::ARef, //
};

pub fn erase<P: Policy>(authority: &Authority<P>) -> ARef<Revocation> {
    authority.revocation()
}

#[cfg(negative)]
pub fn substitute<P: Policy, Q: Policy>(authority: &Authority<P>) -> ARef<Authority<Q>> {
    authority.revocation()
}
