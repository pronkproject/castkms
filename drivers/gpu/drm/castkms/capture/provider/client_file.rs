// SPDX-License-Identifier: GPL-2.0-only

//! Capture-client transport retains permission, not the grantor's revocation lifetime.

use super::Capture;
use kernel::{
    drm::capture::ClientOwner,
    fs::File,
    prelude::*,
    sync::aref::ARef, //
};

// SAFETY: Capture's release trampoline and permission destruction belong to CastKMS.
#[vtable]
unsafe impl ClientOwner for Capture {}

impl Capture {
    /// Transfer this handle without retaining its grantor or the DRM file that created it.
    ///
    /// Closing the client releases only this handle; sibling clients remain authorized until
    /// the independent grantor, creator or device revokes them. Creation failure drops this
    /// handle too, without asking for revocation. No descriptor or pixel operation is exposed.
    /// Call outside DRM, admission and provider cleanup locks.
    pub(crate) fn into_client_file(self) -> Result<ARef<File>> {
        let authority = self.authority.clone();
        authority.create_client_file(self)
    }
}
