// SPDX-License-Identifier: GPL-2.0-only

//! Assemble independent renderer access and revocation file lifetimes.

use super::{
    client_file,
    permission::Owner,
    revoker_file,
    session::Session, //
};
use kernel::{
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
    pub(crate) fn new(owner: Owner) -> Result<Self> {
        let session = Session::new(owner.access())?;
        let renderer = client_file::create(session.clone())?;
        let revoker = revoker_file::create(owner, session)?;
        Ok(Self { renderer, revoker })
    }

    /// Transfer owned references in renderer, revoker order without installing descriptors.
    pub(crate) fn into_files(self) -> (ARef<File>, ARef<File>) {
        (self.renderer, self.revoker)
    }
}

impl Owner {
    /// Transfer renderer access and revocation ownership into anonymous files.
    pub(crate) fn into_files(self) -> Result<Files> {
        Files::new(self)
    }
}
