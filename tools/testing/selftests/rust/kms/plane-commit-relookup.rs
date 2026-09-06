// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0596\]
#![no_std]

use kernel::drm::kms::plane::*;

pub fn inspect<P: DriverPlane>(commit: PlaneAtomicCommit<'_, P>) {
    let plane = commit.plane();
    let state = commit.take_state();
    let new = state.get_new_plane_state(plane).unwrap();
    let _: &P::State = &new;
}

#[cfg(negative)]
pub fn mutate<P: DriverPlane>(commit: PlaneAtomicCommit<'_, P>) {
    let plane = commit.plane();
    let state = commit.take_state();
    let mut new = state.get_new_plane_state(plane).unwrap();
    let _: &mut P::State = &mut *new;
}
