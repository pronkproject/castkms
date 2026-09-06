// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0596\]
#![no_std]

use kernel::drm::kms::{crtc::*, vblank::*};

pub fn inspect_then_send<C: VblankDriverCrtc>(mut commit: CrtcAtomicCommit<'_, C>) -> bool {
    let (old, new) = commit.old_new_state();
    if let Some(event) = commit.get_pending_vblank_event() {
        event.send();
    }
    old.active() != new.active()
}

#[cfg(negative)]
pub fn mutate<C: DriverCrtc>(commit: CrtcAtomicCommit<'_, C>) {
    let (_, new) = commit.old_new_state();
    let _: &mut C::State = &mut **new;
}
