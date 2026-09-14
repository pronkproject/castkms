// SPDX-License-Identifier: GPL-2.0-only

//! Assemble separate capture and revocation lifetimes for grant publication.

use super::Grantor;
use kernel::{
    drm::capture::FilePair,
    prelude::*, //
};

impl Grantor {
    /// Transfer this grant into checked endpoints without installing descriptors.
    ///
    /// Only the control file retains the grantor and its creator registration. The client
    /// retains permission independently. Failure consumes the grant and revokes it through
    /// normal owner cleanup; call outside DRM, admission and provider cleanup locks.
    /// Pair identity does not replace authorization at issuance or later capture operations.
    pub(crate) fn into_files(self) -> Result<FilePair> {
        let capture = self.capture().into_client_file()?;
        let control = self.into_control_file()?;
        FilePair::from_files(&capture, &control)
    }
}
