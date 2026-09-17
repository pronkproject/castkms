// SPDX-License-Identifier: GPL-2.0-only

//! Assemble independent renderer access and revocation file lifetimes.

use super::{
    client_file,
    permission::Owner,
    revoker_file,
    endpoint::Endpoint, //
};
use kernel::{
    drm::{device::Registered, Device},
    fs::File,
    prelude::*,
    sync::aref::ARef, //
};

/// Renderer access and revocation endpoints belonging to one permission owner.
#[must_use = "dropping the files releases renderer access and revokes its owner"]
pub(crate) struct Files {
    renderer: ARef<File>,
    revoker: ARef<File>,
}

impl Files {
    /// Transfer one owner into separately retained renderer and revocation endpoints.
    ///
    /// The renderer file holds only an access handle. Final revoker release drops the
    /// unique owner and revokes every access handle, including duplicated renderer files.
    /// Failure publishes no descriptor and revokes through normal owner cleanup.
    pub(crate) fn new(owner: Owner, device: &Device<crate::Driver, Registered>) -> Result<Self> {
        let endpoint = Endpoint::new(owner.access(), device.to_registered_ref())?;
        let renderer = client_file::create(endpoint)?;
        let revoker = revoker_file::create(owner)?;
        Ok(Self { renderer, revoker })
    }

    /// Transfer owned references in renderer, revoker order without installing descriptors.
    pub(crate) fn into_files(self) -> (ARef<File>, ARef<File>) {
        (self.renderer, self.revoker)
    }
}
