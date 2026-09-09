// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Preassembled admission and completion ownership, below display transaction acceptance.

use super::PreparedRetirement;
use crate::{
    dma_fence::Fence,
    error::from_err_ptr,
    prelude::*, //
};
use core::ptr::NonNull;

/// A movable owner of source admission holds and their collected native completion.
///
/// Construction performs all allocation and fence collection. Moving the owner and borrowing
/// completion do neither. Creation borrows preparation, so failure leaves it available for retry.
/// Dropping the guard releases its holds without waiting for or canceling native readers.
/// All operations, including destruction, require a context that may sleep.
///
/// This is not proof of transaction acceptance or permission to release pixel storage. A display
/// transaction must validate its scope and transfer the owner at its own acceptance boundary.
///
/// # Invariants
///
/// The pointer owns one initialized native guard, released exactly once by Drop.
pub struct RetirementGuard {
    raw: NonNull<bindings::drm_prepare_retirement_guard>,
}

// SAFETY: Moving the unique guard transfers immutable ownership; native references are thread-safe.
unsafe impl Send for RetirementGuard {}
// SAFETY: Shared access only borrows the guard's retained native completion.
unsafe impl Sync for RetirementGuard {}

impl RetirementGuard {
    /// Collect completion and retain admission without consuming the preparation owner.
    pub fn new(prepared: &PreparedRetirement) -> Result<Self> {
        // SAFETY: The borrowed prepared owner retains a ready native set throughout construction.
        let raw = from_err_ptr(unsafe {
            bindings::drm_prepare_retirement_guard_create(prepared.as_raw_set())
        })?;
        Ok(Self {
            // SAFETY: Successful creation returns one non-null, uniquely owned guard.
            raw: unsafe { NonNull::new_unchecked(raw) },
        })
    }

    /// Borrow collected completion without allocating or rescanning source accounting.
    ///
    /// `None` means no native wait was needed at construction. Completion does not prove valid
    /// pixels. Retaining a fence independently does not retain the guard's admission holds.
    pub fn completion(&self) -> Option<&Fence> {
        // SAFETY: The guard is initialized and retained throughout the shared borrow.
        let raw = unsafe { bindings::drm_prepare_retirement_guard_completion(self.raw.as_ptr()) };
        // SAFETY: The native guard retains every non-null result for the lifetime of self.
        NonNull::new(raw).map(|fence| unsafe { Fence::from_raw(fence.as_ptr()) })
    }
}

impl Drop for RetirementGuard {
    fn drop(&mut self) {
        // SAFETY: Drop consumes the unique native guard and releases its retained references.
        unsafe { bindings::drm_prepare_retirement_guard_destroy(self.raw.as_ptr()) };
    }
}
