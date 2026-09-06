// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0308\]
#![no_std]
mod common;

#[cfg(negative)]
pub fn skip<'a, D: kernel::drm::kms::KmsDriver>(
    mut tail: kernel::drm::kms::atomic::AtomicCommitTail<'a, D>,
    ready: kernel::drm::kms::atomic::ModesetsReadyToken<'a, D>,
) {
    let _ = tail.commit_modeset_enables(ready);
}
