// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0521\]
#![no_std]

use kernel::{
    drm::kms::{atomic::*, crtc::*, KmsDriver},
    prelude::*,
};

pub fn check<C: DriverCrtc>(check: CrtcAtomicCheck<'_, C>) -> Result {
    check.take_state().try_for_each_new_crtc_state(|_, state| {
        core::hint::black_box(state);
    })
}

pub fn compose<D: KmsDriver>(state: &mut AtomicStateComposer<D>) -> Result {
    state.try_for_each_new_crtc_state(|_, new| {
        core::hint::black_box(new);
    })
}

#[cfg(negative)]
pub fn escape<D: KmsDriver>(state: &AtomicStateComposer<D>) {
    let mut escaped = None;
    let _ = state.try_for_each_new_crtc_state(|_, new| {
        escaped = Some(new);
    });
    core::hint::black_box(escaped);
}
