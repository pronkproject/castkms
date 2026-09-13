// SPDX-License-Identifier: GPL-2.0-only

//! Standard DRM property transport for a kernel execution description.

use super::{
    Description,
    Profile, //
};
use crate::display::Connector;
use kernel::{
    drm::kms::connector::UnregisteredConnector,
    prelude::*,
    uapi, //
};

fn encode(description: Description) -> [u8; 16] {
    let mut bytes = [0; 16];
    let profile = match description.profile {
        Profile::HostV1 => uapi::DRM_CASTKMS_EXECUTION_HOST_V1,
    };
    bytes[0..4].copy_from_slice(&uapi::DRM_CASTKMS_EXECUTION_VERSION.to_ne_bytes());
    bytes[4..8].copy_from_slice(&profile.to_ne_bytes());
    bytes[8..16].copy_from_slice(&description.generation.to_ne_bytes());
    bytes
}

/// Publish only the fixed initial profile; no runtime capability transition is exposed.
pub(crate) fn attach(connector: &UnregisteredConnector<Connector>) -> Result {
    const { assert!(core::mem::size_of::<uapi::drm_castkms_execution>() == 16) };
    connector.attach_static_blob_property(c"CASTKMS_EXECUTION", &encode(super::describe()))
}
