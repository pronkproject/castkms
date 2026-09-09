// SPDX-License-Identifier: GPL-2.0 OR MIT

//! File transport over preparation ownership, without display policy or descriptor installation.

use super::Ticket;
use crate::{
    error::from_err_ptr,
    fs::{
        File,
        LocalFile, //
    },
    prelude::*,
    sync::aref::ARef, //
};
use core::ptr::NonNull;

impl Ticket {
    /// Create an owned, poll-only file whose final release cancels this ticket.
    ///
    /// File references share one lifetime, independently of kernel ticket references. File
    /// release may be deferred, and closing one descriptor does not revoke copies or active
    /// operations. Use [`Self::cancel`] for prompt cancellation. Even an unpublished file
    /// cancels on final release. Creation failure leaves the ticket unchanged.
    ///
    /// No descriptor is installed and no pixel or modesetting access is granted. An issuer
    /// must establish source authority and finish fallible setup before descriptor publication.
    pub fn create_file(&self) -> Result<ARef<File>> {
        // SAFETY: The borrowed ticket remains initialized and native creation retains it.
        let file =
            from_err_ptr(unsafe { bindings::drm_prepare_ticket_file_create(self.as_raw()) })?;
        // SAFETY: Native creation transfers an initialized file reference. The file is not yet
        // published, so no fdget_pos operation can violate the thread-safe File invariant.
        Ok(unsafe { ARef::from_raw(NonNull::new_unchecked(file.cast())) })
    }

    /// Retain the ticket from a live preparation file, rejecting other file types.
    ///
    /// Lookup does not reserve or validate source scope. The resulting ticket remains subject
    /// to cancellation when the file's final reference is released.
    pub fn from_file(file: &LocalFile) -> Result<ARef<Self>> {
        // SAFETY: The borrowed file remains live; native lookup checks its operations table
        // before retaining a ticket. No file-position state is read or modified.
        let ticket =
            from_err_ptr(unsafe { bindings::drm_prepare_ticket_file_get_ticket(file.as_ptr()) })?;
        // SAFETY: Successful lookup transfers a non-null initialized ticket reference.
        Ok(unsafe { ARef::from_raw(NonNull::new_unchecked(ticket.cast())) })
    }
}
