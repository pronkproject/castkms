// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Cancelable request ownership above source admission accounting.

use super::RetirementSet;
use crate::{
    error::from_err_ptr,
    prelude::*,
    sync::aref::{
        ARef,
        AlwaysRefCounted, //
    },
    types::Opaque, //
};
use core::ptr::NonNull;

/// A shared, explicitly cancelable owner of a source retirement set.
///
/// Dropping a reference is not cancellation. A transport or authority owner must call
/// [`Self::cancel`] at its specified lifetime boundary. Cancellation cannot release ownership
/// retained by an active attempt or accepted display update. The ticket does not authenticate
/// callers, validate display scope or grant pixel access. All operations may sleep.
///
/// # Invariants
///
/// Every reference retains an initialized native ticket with a live reference count.
#[repr(transparent)]
pub struct Ticket(Opaque<bindings::drm_prepare_ticket>);

// SAFETY: Native reference counting and cancellation support ownership across tasks.
unsafe impl Send for Ticket {}
// SAFETY: Shared access exposes only mutex-protected cancellation and atomic reference counting.
unsafe impl Sync for Ticket {}

// SAFETY: Native get/put retain and release the initialized ticket allocation.
unsafe impl AlwaysRefCounted for Ticket {
    fn inc_ref(&self) {
        // SAFETY: The shared reference retains a live initialized ticket.
        unsafe { bindings::drm_prepare_ticket_get(self.0.get()) };
    }

    unsafe fn dec_ref(ptr: NonNull<Self>) {
        // SAFETY: The caller transfers one reference to the identically represented ticket.
        unsafe { bindings::drm_prepare_ticket_put(ptr.as_ptr().cast()) };
    }
}

impl Ticket {
    /// Retain a set independently of the caller's original ownership.
    ///
    /// Read claims may still be pending. Ticket construction does not establish readiness or
    /// wait for completion; reservation checks that condition before creating an attempt.
    pub fn new(set: &RetirementSet) -> Result<ARef<Self>> {
        // SAFETY: The borrowed set remains initialized throughout native reference acquisition.
        let ticket = from_err_ptr(unsafe { bindings::drm_prepare_ticket_create(set.as_raw()) })?;
        // SAFETY: Successful creation transfers a non-null initialized native reference.
        Ok(unsafe { ARef::from_raw(NonNull::new_unchecked(ticket.cast())) })
    }

    /// Prevent new acceptance without canceling already submitted native reads.
    ///
    /// Repeated cancellation is harmless. An accepted guard remains independently owned.
    pub fn cancel(&self) {
        // SAFETY: The shared reference retains the ticket; native cancellation is serialized.
        unsafe { bindings::drm_prepare_ticket_cancel(self.0.get()) };
    }
}

#[cfg(CONFIG_KUNIT)]
mod tests;
