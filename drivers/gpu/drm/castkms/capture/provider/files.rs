// SPDX-License-Identifier: GPL-2.0-only

//! Assemble separate capture and revocation lifetimes for grant publication.

use super::{
    Capture,
    Grantor, //
};
use kernel::{
    drm::capture::{
        ClientOwner,
        FilePair, //
    },
    prelude::*, //
};

impl Grantor {
    /// Transfer this grant into checked endpoints without installing descriptors.
    ///
    /// Only the control file retains the grantor and its creator registration. The client
    /// retains permission independently. Failure consumes the grant and revokes it through
    /// normal owner cleanup; call outside DRM, admission and provider cleanup locks.
    /// Pair identity does not replace authorization at issuance or later capture operations.
    #[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
    pub(crate) fn into_files(self) -> Result<FilePair> {
        self.into_files_with(Ok)
    }

    /// Attach operation state without making the provider depend on a client adapter.
    ///
    /// The builder receives a capture handle; the control endpoint alone retains this
    /// grantor. Failure while building or assembling files consumes and revokes the grant,
    /// even if other capture handles survive. Call outside all provider cleanup locks.
    pub(crate) fn into_files_with<O: ClientOwner>(
        self,
        build: impl FnOnce(Capture) -> Result<O>,
    ) -> Result<FilePair> {
        let capture = self.capture().into_client_file_with(build)?;
        let control = self.into_control_file()?;
        FilePair::from_files(&capture, &control)
    }
}
