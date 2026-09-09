// SPDX-License-Identifier: GPL-2.0 OR MIT
// error-pattern: use of moved value: `attempt`
#![no_std]

use kernel::drm::preparation::Attempt;

pub fn transfer(attempt: Attempt) -> Option<Attempt> {
    let destination = Some(attempt);
    #[cfg(negative)]
    drop(attempt);
    destination
}
