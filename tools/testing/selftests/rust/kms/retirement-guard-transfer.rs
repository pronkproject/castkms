// SPDX-License-Identifier: GPL-2.0 OR MIT
// error-pattern: use of moved value: `guard`
#![no_std]

use kernel::drm::preparation::RetirementGuard;

pub fn transfer(guard: RetirementGuard) -> Option<RetirementGuard> {
    let destination = Some(guard);
    #[cfg(negative)]
    drop(guard);
    destination
}
