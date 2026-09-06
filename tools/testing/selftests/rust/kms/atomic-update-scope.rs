// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0521\]
#![no_std]

use kernel::drm::{
    device::Registered,
    kms::{crtc::Crtc, KmsDriver},
    Device,
};

pub fn scoped<D: KmsDriver>(dev: &Device<D, Registered>, crtc: &Crtc<D::Crtc>) {
    let _ = dev.atomic_update(|state| {
        let mut new = state.add_crtc_state(crtc)?;
        new.set_mode_changed(true);
        Ok(())
    });
}

#[cfg(negative)]
pub fn escape<D: KmsDriver>(dev: &Device<D, Registered>, crtc: &Crtc<D::Crtc>) {
    let mut escaped = None;
    let _ = dev.atomic_update(|state| {
        escaped = Some(state.add_crtc_state(crtc)?);
        Ok(())
    });
    drop(escaped);
}
