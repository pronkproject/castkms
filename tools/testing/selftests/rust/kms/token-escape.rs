// SPDX-License-Identifier: GPL-2.0
// error-pattern: error: lifetime may not live long enough
#![no_std]
mod common;

#[cfg(negative)]
pub fn escape<'a, D: kernel::drm::kms::KmsDriver>(
    mut tail: kernel::drm::kms::atomic::AtomicCommitTail<'a, D>,
    token: kernel::drm::kms::atomic::ModesetsReadyToken<'a, D>,
) -> kernel::drm::kms::atomic::DisablesCommittedToken<'static, D> {
    tail.commit_modeset_disables(token)
}
