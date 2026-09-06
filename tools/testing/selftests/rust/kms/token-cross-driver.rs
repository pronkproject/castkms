// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0308\]
#![no_std]
mod common;

#[cfg(negative)]
pub fn substitute<'a, D: kernel::drm::kms::KmsDriver, E: kernel::drm::kms::KmsDriver>(
    mut tail: kernel::drm::kms::atomic::AtomicCommitTail<'a, D>,
    unrelated: kernel::drm::kms::atomic::ModesetsReadyToken<'a, E>,
) {
    let _ = tail.commit_modeset_disables(unrelated);
}
