// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Checked capture and control endpoints, without granting or publishing authority.

use crate::{
    fs::File,
    prelude::*,
    sync::aref::ARef, //
};

/// A capture-client file and a control file belonging to the same authority.
///
/// Retaining this pair shares the files' existing ownership; it creates neither another
/// grant nor another revocation owner. Matching proves roles and identity, not permission.
/// A revoked pair still matches. Releasing the last control-file reference revokes normally.
///
/// # Invariants
///
/// Both files retain their native endpoint types and refer to the same authority.
#[must_use = "dropping the pair releases its capture and control references"]
pub struct FilePair {
    capture: ARef<File>,
    control: ARef<File>,
}

impl FilePair {
    /// Retain matching borrowed files without changing their policy or failure ownership.
    ///
    /// Failure leaves both callers' references intact and does not request revocation.
    /// Success installs no descriptor. A publishing adapter must separately authorize
    /// issuance and finish fallible output work before transferring reserved descriptors.
    pub fn from_files(capture: &File, control: &File) -> Result<Self> {
        // SAFETY: Both references retain initialized files. Native matching validates each
        // immutable operations table before reading endpoint data and changes no ownership.
        if !unsafe { bindings::drm_capture_files_match(capture.as_ptr(), control.as_ptr()) } {
            return Err(EINVAL);
        }
        Ok(Self {
            capture: capture.into(),
            control: control.into(),
        })
    }

    /// Borrow the capture-client file without adding any pixel or primary-node operation.
    pub fn capture_file(&self) -> &File {
        &self.capture
    }

    /// Borrow the separate revocation endpoint, sharing its existing close lifetime.
    pub fn control_file(&self) -> &File {
        &self.control
    }

    /// Transfer owned references in capture, control order, without publishing descriptors.
    pub fn into_files(self) -> (ARef<File>, ARef<File>) {
        (self.capture, self.control)
    }
}

#[cfg(CONFIG_KUNIT)]
mod tests;
