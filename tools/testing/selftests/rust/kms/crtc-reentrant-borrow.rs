// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0502\]
#![no_std]

use kernel::drm::kms::{atomic::*, crtc::*, KmsDriver};

pub fn read<D: KmsDriver>(state: &mut AtomicStateMutator<D>) {
    state.for_each_new_crtc_state(|_, shared| {
        core::hint::black_box(shared);
    });
}

#[cfg(negative)]
pub fn reentrant<D: KmsDriver>(state: &mut AtomicStateMutator<D>) {
    state.for_each_new_crtc_state(|crtc, opaque| {
        let shared = CrtcState::<<D::Crtc as DriverCrtc>::State>::from_opaque(opaque);
        let mut guard = state.get_new_crtc_state(crtc).unwrap();
        core::hint::black_box(&mut *guard);
        core::hint::black_box(&**shared);
    });
}
