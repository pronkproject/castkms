// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0502\]
#![no_std]

use core::pin::Pin;
use kernel::drm::kms::{atomic::AtomicStateComposer, crtc::Crtc, KmsDriver};

pub fn configure<D: KmsDriver>(mut state: Pin<&mut AtomicStateComposer<D>>, crtc: &Crtc<D::Crtc>) {
    let _ = state.as_mut().set_crtc_config(crtc, None);
    let _ = state.add_crtc_state(crtc);
}

#[cfg(negative)]
pub fn outstanding_guard<D: KmsDriver>(
    mut state: Pin<&mut AtomicStateComposer<D>>,
    crtc: &Crtc<D::Crtc>,
) {
    let guard = state.add_crtc_state(crtc);
    let _ = state.as_mut().set_crtc_config(crtc, None);
    drop(guard);
}
