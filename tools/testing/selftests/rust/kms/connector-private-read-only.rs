// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0596\]
#![no_std]

use kernel::drm::kms::connector::{ConnectorState, DriverConnectorState};

pub fn read<S: DriverConnectorState>(state: &ConnectorState<S>) -> &S {
    state
}

#[cfg(negative)]
pub fn mutate<S: DriverConnectorState>(state: &ConnectorState<S>) -> &mut S {
    &mut **state
}
