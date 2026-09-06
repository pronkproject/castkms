// SPDX-License-Identifier: GPL-2.0
// error-pattern: error: lifetime may not live long enough
#![no_std]
mod common;

use kernel::drm::kms::{atomic::CommittedAtomicState, KmsDriver};

#[cfg(not(negative))]
pub fn preserve<'a, D: KmsDriver>(state: CommittedAtomicState<'a, D>) -> CommittedAtomicState<'a, D> {
    state
}

#[cfg(negative)]
pub fn substitute<'outer: 'inner, 'inner, D: KmsDriver>(
    state: CommittedAtomicState<'outer, D>,
) -> CommittedAtomicState<'inner, D> {
    state
}
