// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0599\]
#![no_std]

use kernel::drm::kms::{
    crtc::{
        Crtc,
        DriverCrtc, //
    },
    vblank::{
        SoftwareVblank,
        VblankDriverCrtc,
        VblankSample, //
    }, //
};

pub fn observe<C: VblankDriverCrtc>(crtc: &Crtc<C>) -> VblankSample {
    crtc.vblank_count_and_time()
}

pub fn software<C: DriverCrtc<VblankImpl = SoftwareVblank<C>>>(crtc: &Crtc<C>) {
    let sample = observe(crtc);
    let _ = sample.sequence();
    let _ = sample.timestamp();
}

#[cfg(negative)]
pub fn unsupported<C: DriverCrtc<VblankImpl = core::marker::PhantomData<C>>>(crtc: &Crtc<C>) {
    let _ = crtc.vblank_count_and_time();
}
