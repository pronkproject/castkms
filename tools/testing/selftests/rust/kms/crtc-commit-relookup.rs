// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0596\]
#![no_std]

use kernel::drm::kms::crtc::*;

pub fn inspect<C: DriverCrtc>(commit: CrtcAtomicCommit<'_, C>) {
    let crtc = commit.crtc();
    let state = commit.take_state();
    let new = state.get_new_crtc_state(crtc).unwrap();
    let _: &C::State = &new;
}

#[cfg(negative)]
pub fn mutate<C: DriverCrtc>(commit: CrtcAtomicCommit<'_, C>) {
    let crtc = commit.crtc();
    let state = commit.take_state();
    let mut new = state.get_new_crtc_state(crtc).unwrap();
    let _: &mut C::State = &mut *new;
}
