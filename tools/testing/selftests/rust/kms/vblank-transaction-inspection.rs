// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0599\]
#![no_std]

use kernel::drm::kms::{
    crtc::{
        CrtcAtomicCommit,
        DriverCrtc, //
    },
    vblank::VblankDriverCrtc, //
};

pub fn inspect_then_send<C: VblankDriverCrtc>(mut commit: CrtcAtomicCommit<'_, C>) -> bool {
    let state = commit.atomic_state();
    let crtc = commit.crtc();
    let primary_present = state.get_new_plane_state(crtc.primary_plane()).is_some();
    if let Some(event) = commit.get_pending_vblank_event() {
        event.send();
    }
    primary_present && state.get_new_crtc_state(crtc).is_some()
}

#[cfg(negative)]
pub fn mutate<C: DriverCrtc>(commit: CrtcAtomicCommit<'_, C>) {
    let _ = commit.atomic_state().add_crtc_state(commit.crtc());
}
