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
        VblankDriverCrtc, //
    },
};

pub fn supported<C: DriverCrtc<VblankImpl = SoftwareVblank<C>>>(crtc: &Crtc<C>) {
    fn requires_support<T: VblankDriverCrtc>() {}
    requires_support::<C>();
    let _ = crtc.vblank_get();
}

#[cfg(negative)]
pub fn bypass_native_callback<C: DriverCrtc<VblankImpl = SoftwareVblank<C>>>() {
    use kernel::drm::kms::vblank::VblankSupport;

    let _ = SoftwareVblank::<C>::enable_vblank;
}
