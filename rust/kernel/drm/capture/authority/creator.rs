// SPDX-License-Identifier: GPL-2.0 OR MIT

//! A unique creator lifetime, independent of retained capture authority.

use super::{
    Authority,
    Policy, //
};
use crate::{
    error::from_err_ptr,
    prelude::*, //
};
use core::ptr::NonNull;

/// Closing this owner revokes its remaining grants, without waiting for GPU completion.
///
/// All operations and destruction may sleep. Call outside authority cleanup locks.
///
/// # Invariants
///
/// The pointer owns the unique close obligation for an initialized native creator.
#[must_use = "dropping the creator revokes its tracked grants"]
pub struct Creator(NonNull<bindings::drm_capture_creator>);

// SAFETY: Native tracking is synchronized; ownership may move between tasks.
unsafe impl Send for Creator {}
// SAFETY: Native registration is serialized, and close requires exclusive ownership.
unsafe impl Sync for Creator {}

impl Creator {
    /// Allocate tracking storage with a provider-selected maximum number of grants.
    pub fn new(limit: u32) -> Result<Self> {
        // SAFETY: The native constructor accepts a scalar and transfers an owned reference.
        let raw = from_err_ptr(unsafe { bindings::drm_capture_creator_create(limit) })?;
        Ok(Self(NonNull::new(raw).ok_or(ENOMEM)?))
    }

    /// Track an already-authorized grant without extending the creator's lifetime.
    pub fn register<P: Policy>(&self, authority: &Authority<P>) -> Result<Registration> {
        // SAFETY: Both objects remain live across registration. Borrowing self excludes close.
        let raw = from_err_ptr(unsafe {
            bindings::drm_capture_creator_register(self.0.as_ptr(), authority.raw.get())
        })?;
        Ok(Registration(NonNull::new(raw).ok_or(ENOMEM)?))
    }
}

impl Drop for Creator {
    fn drop(&mut self) {
        // SAFETY: Unique ownership transfers the creator reference; registrations retain storage.
        unsafe { bindings::drm_capture_creator_close(self.0.as_ptr()) };
    }
}

/// A tracked grant; dropping it removes tracking without revoking the authority.
///
/// This handle may outlive its creator. Destruction may sleep and release provider policy.
///
/// # Invariants
///
/// The pointer owns one removal obligation returned by native registration.
#[must_use = "dropping the registration removes creator lifetime tracking"]
pub struct Registration(NonNull<bindings::drm_capture_registration>);

// SAFETY: Native removal is serialized against creator close and owns its tracking references.
unsafe impl Send for Registration {}
// SAFETY: Shared access exposes no operations; removal requires exclusive ownership.
unsafe impl Sync for Registration {}

impl Drop for Registration {
    fn drop(&mut self) {
        // SAFETY: This handle uniquely owns the returned registration reference.
        unsafe { bindings::drm_capture_registration_remove(self.0.as_ptr()) };
    }
}
