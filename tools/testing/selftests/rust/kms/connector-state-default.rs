// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0277\]
#![no_std]

use kernel::drm::kms::connector::*;

// Driver-private defaults remain optional; only DRM constructs the wrapper
// that promises an initialized parent connector.
pub fn private_default<S: DriverConnectorState + Default>() -> S {
    S::default()
}

pub fn parent<S: DriverConnectorState>(
    state: &ConnectorState<S>,
) -> &Connector<S::Connector> {
    state.connector()
}

#[cfg(negative)]
pub fn parentless<S: DriverConnectorState>() -> ConnectorState<S> {
    Default::default()
}
