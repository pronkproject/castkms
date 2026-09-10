// SPDX-License-Identifier: GPL-2.0

//! File transport for an existing native completion record.

use super::Fence;
use crate::{
    fs::File,
    prelude::*,
    sync::aref::ARef, //
};
use core::ptr::NonNull;

impl Fence {
    /// Create an owned sync file retaining this exact completion record.
    ///
    /// Creation neither waits nor installs a descriptor. Pending, successful and failed
    /// records remain observable through the native sync-file interface. Poll readiness means
    /// completion, not successful pixel production; the original error status is preserved.
    ///
    /// The caller must finish authorization and fallible result construction before publishing
    /// a descriptor, using close-on-exec descriptor reservation as appropriate. The file grants
    /// no buffer access and does not close source-read admission or represent future work.
    /// Dropping unpublished or duplicated file references does not signal the underlying fence.
    pub fn create_sync_file(&self) -> Result<ARef<File>> {
        // SAFETY: The borrowed fence remains live. Native creation retains its own fence
        // reference on success and returns null on failure without consuming the caller's.
        let sync =
            NonNull::new(unsafe { bindings::sync_file_create(self.as_raw()) }).ok_or(ENOMEM)?;
        // SAFETY: The new sync file owns one initialized, unpublished file reference. No
        // fdget_pos call exists on that file, satisfying the thread-safe File invariant.
        let file = unsafe { (*sync.as_ptr()).file };
        // SAFETY: Successful sync_file_create guarantees a non-null file and transfers its
        // reference to the caller. File's transparent representation preserves that ownership.
        Ok(unsafe { ARef::from_raw(NonNull::new_unchecked(file.cast())) })
    }
}

#[cfg(CONFIG_KUNIT)]
mod tests;
