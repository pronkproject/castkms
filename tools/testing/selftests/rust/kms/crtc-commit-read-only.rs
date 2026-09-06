// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0596\]
#![no_std]

use kernel::drm::kms::crtc::*;

pub fn inspect<C: DriverCrtc>(commit: CrtcAtomicCommit<'_, C>) {
    let (_, old, new) = commit.take_all();
    let _: &C::State = &old;
    let _: &C::State = &new;
}

#[cfg(negative)]
pub fn mutate<C: DriverCrtc>(commit: CrtcAtomicCommit<'_, C>) {
    let (_, _, mut new) = commit.take_all();
    let _: &mut C::State = &mut *new;
}
