// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0599\]
#![no_std]

use kernel::drm::kms::{crtc::*, vblank::*};

pub fn after_acceptance<C: VblankDriverCrtc>(mut commit: CrtcAtomicCommit<'_, C>) {
    if let Some(event) = commit.get_pending_vblank_event() {
        event.send();
    }
}

#[cfg(negative)]
pub fn before_acceptance<C: VblankDriverCrtc>(check: CrtcAtomicCheck<'_, C>) {
    let mut state = check.take_new_state();
    let _ = state.get_pending_vblank_event();
}
