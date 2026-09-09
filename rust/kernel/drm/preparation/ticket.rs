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
// SAFETY: Shared operations serialize cancellation and reservation through the native mutex.
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

    /// Reserve one attempt without consuming the cancelable request.
    ///
    /// Pending claims return EAGAIN, another reservation returns EBUSY, and cancellation returns
    /// ECANCELED. All native completion collection precedes publication. Dropping the returned
    /// owner permits retry if the ticket is still live; it never invents successful installation.
    pub fn reserve(&self) -> Result<Attempt> {
        // SAFETY: The shared ticket remains live; reservation takes its own native reference.
        let attempt = from_err_ptr(unsafe { bindings::drm_prepare_ticket_reserve(self.0.get()) })?;
        Ok(Attempt {
            // SAFETY: Successful reservation transfers one non-null, uniquely owned attempt.
            raw: unsafe { NonNull::new_unchecked(attempt) },
        })
    }
}

/// A unique reservation retaining its ticket and preassembled retirement ownership.
///
/// Moving an attempt does not release admission. Dropping it abandons only that attempt,
/// permitting a still-live ticket to retry. Cancellation may prevent acceptance but cannot
/// release this owner's holds. No public method accepts a display update or exposes raw pointers.
/// Destruction requires a context that may sleep.
///
/// # Invariants
///
/// The pointer owns one initialized native attempt, released exactly once by Drop.
pub struct Attempt {
    raw: NonNull<bindings::drm_prepare_attempt>,
}

// SAFETY: Exclusive ownership may move between tasks; native ticket access uses its own mutex.
unsafe impl Send for Attempt {}

impl Drop for Attempt {
    fn drop(&mut self) {
        // SAFETY: Drop exclusively consumes the native attempt and its retained references.
        unsafe { bindings::drm_prepare_attempt_destroy(self.raw.as_ptr()) };
    }
}

#[cfg(CONFIG_KUNIT)]
mod tests;
