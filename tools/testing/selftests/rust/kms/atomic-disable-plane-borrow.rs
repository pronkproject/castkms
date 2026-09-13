// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0502\]
#![no_std]

use core::pin::Pin;
use kernel::drm::kms::{atomic::AtomicStateComposer, plane::Plane, KmsDriver};

pub fn disable<D: KmsDriver>(mut state: Pin<&mut AtomicStateComposer<D>>, plane: &Plane<D::Plane>) {
    let _ = state.as_mut().disable_plane(plane);
    let _ = state.add_plane_state(plane);
}

#[cfg(negative)]
pub fn outstanding_guard<D: KmsDriver>(
    mut state: Pin<&mut AtomicStateComposer<D>>,
    plane: &Plane<D::Plane>,
) {
    let guard = state.add_plane_state(plane);
    let _ = state.as_mut().disable_plane(plane);
    drop(guard);
}
