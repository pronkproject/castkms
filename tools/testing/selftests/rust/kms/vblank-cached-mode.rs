// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0599\]
#![no_std]

use kernel::drm::kms::{crtc::*, vblank::*};

pub fn transaction<C: DriverCrtc>(commit: CrtcAtomicCommit<'_, C>) -> i32 {
    commit.take_new_state().adjusted_mode().crtc_clock()
}

#[cfg(negative)]
pub fn unrelated_lock<C: VblankDriverCrtc>(guard: &VblankGuard<'_, C>) {
    let _ = guard.hwmode();
}
