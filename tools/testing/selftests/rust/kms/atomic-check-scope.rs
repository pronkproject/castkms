// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0521\]
#![no_std]

use kernel::drm::{
    device::Registered,
    kms::{crtc::Crtc, KmsDriver},
    Device,
};

pub fn scoped<D: KmsDriver>(dev: &Device<D, Registered>, crtc: &Crtc<D::Crtc>) {
    let _ = dev.check_atomic_update(|state| {
        let _new = state.add_crtc_state(crtc)?;
        Ok(())
    });
}

#[cfg(negative)]
pub fn escape<D: KmsDriver>(dev: &Device<D, Registered>, crtc: &Crtc<D::Crtc>) {
    let mut escaped = None;
    let _ = dev.check_atomic_update(|state| {
        escaped = Some(state.add_crtc_state(crtc)?);
        Ok(())
    });
    drop(escaped);
}
