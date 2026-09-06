// SPDX-License-Identifier: GPL-2.0
// error-pattern: error: lifetime may not live long enough
#![no_std]
mod common;

#[cfg(negative)]
pub fn substitute<'a, 'b, D: kernel::drm::kms::KmsDriver>(
    mut tail: kernel::drm::kms::atomic::AtomicCommitTail<'a, D>,
    unrelated: kernel::drm::kms::atomic::ModesetsReadyToken<'b, D>,
) {
    let _ = tail.commit_modeset_disables(unrelated);
}
