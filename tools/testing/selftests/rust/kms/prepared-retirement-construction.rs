// SPDX-License-Identifier: GPL-2.0 OR MIT
// error-pattern: field `set` of struct `PreparedRetirement` is private
#![no_std]

use kernel::{
    drm::preparation::{PreparedRetirement, RetirementSet},
    error::Result,
    sync::aref::ARef,
};

pub fn prepare(set: ARef<RetirementSet>) -> Result<Option<PreparedRetirement>> {
    #[cfg(negative)]
    return Ok(Some(PreparedRetirement { set }));
    #[cfg(not(negative))]
    set.prepared()
}
