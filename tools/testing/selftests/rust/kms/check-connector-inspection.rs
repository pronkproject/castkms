// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0521\]
#![no_std]

use kernel::{
    drm::kms::{atomic::*, crtc::*, KmsDriver},
    prelude::*,
};

pub fn check<C: DriverCrtc>(check: CrtcAtomicCheck<'_, C>, crtc: &Crtc<C>) -> Result {
    check
        .take_state()
        .with_new_connector_state_for_crtc(crtc, |new| {
            core::hint::black_box(new);
        })
}

pub fn compose<D: KmsDriver>(state: &mut AtomicStateComposer<D>, crtc: &Crtc<D::Crtc>) -> Result {
    state.with_new_connector_state_for_crtc(crtc, |new| {
        core::hint::black_box(new);
    })
}

#[cfg(negative)]
pub fn escape<D: KmsDriver>(state: &AtomicStateComposer<D>, crtc: &Crtc<D::Crtc>) {
    let mut escaped = None;
    let _ = state.with_new_connector_state_for_crtc(crtc, |new| {
        escaped = new;
    });
    core::hint::black_box(escaped);
}
