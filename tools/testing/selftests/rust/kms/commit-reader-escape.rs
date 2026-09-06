// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0277\]
#![no_std]

use kernel::{drm::kms::{atomic::*, KmsDriver}, sync::aref::ARef};

pub fn borrowed<D: KmsDriver>(state: &AtomicStateReader<D>) -> &AtomicStateReader<D> {
    state
}

#[cfg(negative)]
pub fn retained<D: KmsDriver>(state: &AtomicStateReader<D>) -> ARef<AtomicState<D>> {
    ARef::from(state)
}
