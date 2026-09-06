// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0502\]
#![no_std]

use kernel::drm::kms::{atomic::*, crtc::*, KmsDriver};

pub fn read<D: KmsDriver>(state: &mut AtomicStateMutator<D>) {
    state.for_each_new_crtc_state(|_, opaque| {
        let typed = CrtcState::<<D::Crtc as DriverCrtc>::State>::from_opaque(opaque);
        core::hint::black_box(&**typed);
    });
}

#[cfg(negative)]
pub fn conflicting<D: KmsDriver>(state: &mut AtomicStateMutator<D>, crtc: &Crtc<D::Crtc>) {
    let mut guard = state.get_new_crtc_state(crtc).unwrap();
    let exclusive = &mut *guard;
    state.for_each_new_crtc_state(|_, opaque| {
        let shared = CrtcState::<<D::Crtc as DriverCrtc>::State>::from_opaque(opaque);
        core::hint::black_box(&**shared);
    });
    core::hint::black_box(exclusive);
}
