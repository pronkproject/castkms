// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0596\]
#![no_std]

use kernel::drm::kms::plane::*;

pub fn inspect<P: DriverPlane>(commit: PlaneAtomicCommit<'_, P>) {
    let (_, old, new) = commit.take_all();
    let _: &P::State = &old;
    let _: &P::State = &new;
}

#[cfg(negative)]
pub fn mutate<P: DriverPlane>(commit: PlaneAtomicCommit<'_, P>) {
    let (_, _, mut new) = commit.take_all();
    let _: &mut P::State = &mut *new;
}
