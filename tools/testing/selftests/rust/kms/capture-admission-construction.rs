// SPDX-License-Identifier: GPL-2.0 OR MIT
// error-pattern: fields `authority` and `_task` of struct `Admission` are private
#![no_std]

use kernel::{
    drm::capture::{
        Admission,
        Authority,
        Policy, //
    },
    error::Result, //
};

pub fn hold<P: Policy>(authority: &Authority<P>) -> Result<Admission<'_, P>> {
    #[cfg(negative)]
    return Ok(Admission {
        authority,
        _task: kernel::types::NotThreadSafe,
    });
    #[cfg(not(negative))]
    authority.begin()
}
