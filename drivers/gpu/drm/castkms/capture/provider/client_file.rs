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
    #[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
    pub(crate) fn into_client_file(self) -> Result<ARef<File>> {
        self.into_client_file_with(Ok)
    }

    /// Build client operation state above the provider without exposing its native authority.
    ///
    /// The builder receives only capture permission, never the grantor. It runs outside
    /// provider locks and may fail before a file exists. The native file owns its successful
    /// result until final close. Failure drops permission normally without requesting revoke.
    pub(crate) fn into_client_file_with<O: ClientOwner>(
        self,
        build: impl FnOnce(Self) -> Result<O>,
    ) -> Result<ARef<File>> {
        let authority = self.authority.clone();
        authority.create_client_file(build(self)?)
    }
}
