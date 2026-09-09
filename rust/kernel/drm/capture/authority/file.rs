// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Revocation file transport over provider-owned authority.

use super::{
    Authority,
    Policy, //
};
use crate::{
    error::from_err_ptr,
    fs::File,
    prelude::*,
    sync::aref::ARef, //
};
use core::ptr::NonNull;

impl<P: Policy> Authority<P> {
    /// Create an owned control file whose final release revokes this authority.
    ///
    /// Cloned file references share one revocation lifetime. Independently created files each
    /// revoke on final release, even while other control files or authority references survive.
    /// File release may be deferred; use [`Self::revoke`] for prompt revocation.
    ///
    /// Creation failure leaves authority unchanged. Successfully created files revoke even if
    /// never published. Issuers must finish fallible setup before publishing descriptors and
    /// treat rollback of an owned file as terminal. Do not release it under an admission guard
    /// or locks needed by provider cleanup.
    ///
    /// The file only reports completed revocation through poll. It grants no image, source,
    /// modesetting or mapping access, and this operation installs no descriptor.
    pub fn create_control_file(&self) -> Result<ARef<File>> {
        // SAFETY: The authority remains live; successful native creation retains its own reference.
        let file =
            from_err_ptr(unsafe { bindings::drm_capture_control_file_create(self.raw.get()) })?;
        // SAFETY: Creation transfers a non-null initialized file reference. The unpublished file
        // has no concurrent fdget_pos operation, satisfying the thread-safe File invariant.
        Ok(unsafe { ARef::from_raw(NonNull::new_unchecked(file.cast())) })
    }
}

#[cfg(CONFIG_KUNIT)]
mod tests;
