// SPDX-License-Identifier: GPL-2.0 OR MIT
// error-pattern: cannot be sent between threads safely
#![no_std]

use kernel::{
    drm::capture::{
        Authority,
        Policy, //
    },
    error::Result, //
};

#[cfg(negative)]
fn send<T: Send>(_: T) {}

pub fn hold<P: Policy>(authority: &Authority<P>) -> Result {
    let admission = authority.begin()?;
    #[cfg(negative)]
    send(admission);
    #[cfg(not(negative))]
    drop(admission);
    Ok(())
}
