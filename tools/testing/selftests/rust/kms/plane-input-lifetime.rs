// SPDX-License-Identifier: GPL-2.0
// error-pattern: lifetime may not live long enough
#![no_std]

use kernel::{
    drm::kms::{
        atomic::{AtomicStateMutator, PlaneInput},
        plane::ModesettablePlane,
        KmsDriver, ModeObject,
    },
    prelude::*,
};

pub fn borrowed<'a, D: KmsDriver, P: ModesettablePlane + ModeObject<Driver = D>>(
    state: &'a AtomicStateMutator<D>,
    plane: &P,
) -> Result<PlaneInput<'a, D>> {
    state.plane_input(plane)
}

#[cfg(negative)]
pub fn escaped<D: KmsDriver, P: ModesettablePlane + ModeObject<Driver = D>>(
    state: &AtomicStateMutator<D>,
    plane: &P,
) -> Result<PlaneInput<'static, D>> {
    state.plane_input(plane)
}
