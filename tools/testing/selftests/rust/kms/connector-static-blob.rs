// SPDX-License-Identifier: GPL-2.0
// error-pattern: error\[E0599\]
#![no_std]

use kernel::drm::kms::connector::*;

pub fn during_construction<T: DriverConnector>(
    connector: &UnregisteredConnector<T>,
    bytes: &[u8],
) {
    let _ = connector.attach_static_blob_property(c"description", bytes);
}

#[cfg(negative)]
pub fn after_publication<T: DriverConnector>(connector: &Connector<T>, bytes: &[u8]) {
    let _ = connector.attach_static_blob_property(c"description", bytes);
}
