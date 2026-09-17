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
};

pub(super) fn encode(description: Description) -> [u8; 16] {
    let mut bytes = [0; 16];
    let profile = match description.profile {
        Profile::HostV1 => 1u32,
        Profile::GpuV1 => 2u32,
    };
    bytes[0..4].copy_from_slice(&1u32.to_ne_bytes());
    bytes[4..8].copy_from_slice(&profile.to_ne_bytes());
    bytes[8..16].copy_from_slice(&description.generation.to_ne_bytes());
    bytes
}

/// Attach the initial description before publishing the device.
pub(super) fn attach(
    connector: &UnregisteredConnector<Connector>,
    description: Description,
) -> Result<ReadOnlyBlobProperty<Connector>> {
    connector.attach_readonly_blob_property(c"CASTKMS_EXECUTION", &encode(description))
}
