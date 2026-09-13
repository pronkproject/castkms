// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0277\]
#![no_std]

use kernel::drm::capture::{
    Authority,
    ControlOwner,
    Policy, //
};

pub fn transfer<P: Policy, O: ControlOwner>(authority: &Authority<P>, owner: O) {
    let _ = authority.create_control_file_with_owner(owner);
}

#[cfg(negative)]
pub fn owner_without_module_contract<P: Policy, O: Send + 'static>(
    authority: &Authority<P>,
    owner: O,
) {
    let _ = authority.create_control_file_with_owner(owner);
}
