// SPDX-License-Identifier: GPL-2.0 OR MIT
// error-pattern: cannot be (sent|shared) between threads safely
#![no_std]

use kernel::drm::kms::{atomic::AtomicStateComposer, KmsDriver};

fn sendable<T: Send>(_: T) {}

pub fn scoped<D: KmsDriver + Send + Sync>(state: &mut AtomicStateComposer<D>) {
    #[cfg(not(negative))]
    sendable(state.drm_dev());
    #[cfg(negative)]
    sendable(state);
}
