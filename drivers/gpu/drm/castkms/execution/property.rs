// SPDX-License-Identifier: GPL-2.0-only

//! Standard DRM property transport for a kernel execution description.

use super::{
    Description,
    Profile, //
};
use crate::display::Connector;
use kernel::{
    drm::kms::connector::{
        ReadOnlyBlobProperty,
        UnregisteredConnector, //
    },
    prelude::*,
    uapi, //
};

pub(super) fn encode(description: Description) -> [u8; 16] {
    let mut bytes = [0; 16];
    let profile = match description.profile {
        Profile::HostV1 => uapi::DRM_CASTKMS_EXECUTION_HOST_V1,
    };
    bytes[0..4].copy_from_slice(&uapi::DRM_CASTKMS_EXECUTION_VERSION.to_ne_bytes());
    bytes[4..8].copy_from_slice(&profile.to_ne_bytes());
    bytes[8..16].copy_from_slice(&description.generation.to_ne_bytes());
    bytes
}

/// Attach the initial description before publishing the device.
pub(super) fn attach(
    connector: &UnregisteredConnector<Connector>,
    description: Description,
) -> Result<ReadOnlyBlobProperty<Connector>> {
    const { assert!(core::mem::size_of::<uapi::drm_castkms_execution>() == 16) };
    connector.attach_readonly_blob_property(c"CASTKMS_EXECUTION", &encode(description))
}
