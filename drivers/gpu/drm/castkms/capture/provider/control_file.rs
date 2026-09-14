// SPDX-License-Identifier: GPL-2.0-only

//! Revocation-file transport above the kernel grantor's policy and resource lifetime.

use super::Grantor;
use kernel::{
    drm::capture::ControlOwner,
    fs::File,
    prelude::*,
    sync::aref::ARef, //
};

// SAFETY: Grantor cleanup and its release trampoline belong to CastKMS's local module.
#[vtable]
unsafe impl ControlOwner for Grantor {}

impl Grantor {
    /// Transfer revocation and creator tracking without installing a descriptor.
    ///
    /// Final file release revokes and drops this complete grantor. Capture handles retain
    /// neither owner, and the grantor never retains its creating DRM file. Failure drops
    /// the unpublished grantor and revokes it. Call outside DRM and authority locks.
    /// The returned file only observes revocation; it exposes no capture or source access.
    pub(crate) fn into_control_file(self) -> Result<ARef<File>> {
        let authority = self.capture.authority.clone();
        authority.create_control_file_with_owner(self)
    }
}
