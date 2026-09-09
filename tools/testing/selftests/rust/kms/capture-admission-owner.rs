// SPDX-License-Identifier: GPL-2.0 OR MIT
// error-pattern: cannot move out of `authority` because it is borrowed
#![no_std]

use kernel::{
    drm::capture::{
        Authority,
        Policy, //
    },
    error::Result,
    sync::aref::ARef, //
};

pub fn hold<P: Policy>(authority: ARef<Authority<P>>) -> Result {
    let admission = authority.begin()?;
    #[cfg(negative)]
    drop(authority);
    drop(admission);
    #[cfg(not(negative))]
    drop(authority);
    Ok(())
}
