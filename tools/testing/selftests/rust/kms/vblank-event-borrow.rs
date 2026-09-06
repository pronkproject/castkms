// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0499\]
#![no_std]

use kernel::drm::kms::{crtc::*, vblank::*};

pub fn once<C: VblankDriverCrtc>(mut commit: CrtcAtomicCommit<'_, C>) {
    if let Some(event) = commit.get_pending_vblank_event() {
        event.send();
    }
    let _ = commit.take_all();
}

pub fn arm<C: VblankDriverCrtc>(
    mut commit: CrtcAtomicCommit<'_, C>,
    reference: VblankRef<'_, C>,
) -> kernel::error::Result {
    if let Some(event) = commit.get_pending_vblank_event() {
        event.arm(reference)?;
    }
    Ok(())
}

#[cfg(negative)]
pub fn duplicate<C: VblankDriverCrtc>(mut commit: CrtcAtomicCommit<'_, C>) {
    let first = commit.get_pending_vblank_event().unwrap();
    let second = commit.get_pending_vblank_event().unwrap();
    first.send();
    second.send();
}
