// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Retained result notifications without borrowing a provider's mutable queue.

use crate::{
    error::from_err_ptr,
    fs::File,
    prelude::*,
    sync::aref::{
        ARef,
        AlwaysRefCounted, //
    },
    types::Opaque, //
};
use core::ptr::NonNull;

/// A shared observation that capture results are available, not ownership of those results.
///
/// Providers serialize updates with their result queue. Readers must recheck that queue;
/// readiness does not reserve or acknowledge a result, or grant pixel access.
///
/// # Invariants
///
/// Every reference retains an initialized native readiness object.
#[repr(transparent)]
pub struct Readiness(Opaque<bindings::drm_capture_readiness>);

// SAFETY: The native reference count and readiness operations are synchronized across tasks.
unsafe impl Send for Readiness {}
// SAFETY: Shared references expose only atomic native operations and reference retention.
unsafe impl Sync for Readiness {}

// SAFETY: Native get/put owns the allocation behind every retained reference.
unsafe impl AlwaysRefCounted for Readiness {
    fn inc_ref(&self) {
        // SAFETY: This reference retains a live native object with a nonzero reference count.
        unsafe { bindings::drm_capture_readiness_get(self.as_raw()) };
    }

    unsafe fn dec_ref(ptr: NonNull<Self>) {
        // SAFETY: The caller transfers one reference to the identically represented object.
        unsafe { bindings::drm_capture_readiness_put(ptr.as_ptr().cast()) };
    }
}

impl Readiness {
    /// Retain a client's notification without borrowing its provider or retaining its file.
    ///
    /// A different endpoint returns EINVAL, and absent notification support returns
    /// EOPNOTSUPP. Revocation does not prevent observing retained terminal results.
    pub fn for_client(file: &File) -> Result<ARef<Self>> {
        // SAFETY: The borrow retains the file throughout native endpoint validation and get.
        let raw =
            from_err_ptr(unsafe { bindings::drm_capture_client_get_readiness(file.as_ptr()) })?;
        // SAFETY: Success transfers one non-null reference with identical representation.
        Ok(unsafe { ARef::from_raw(NonNull::new_unchecked(raw.cast())) })
    }

    /// Allocate an independently retained notification, initially reporting no results.
    pub fn new() -> Result<ARef<Self>> {
        // SAFETY: Creation takes no borrowed inputs and returns one reference or an error.
        let raw = from_err_ptr(unsafe { bindings::drm_capture_readiness_create() })?;
        // SAFETY: Success returns a non-null initialized object and transfers its reference.
        Ok(unsafe { ARef::from_raw(NonNull::new_unchecked(raw.cast())) })
    }

    pub(crate) fn as_raw(&self) -> *mut bindings::drm_capture_readiness {
        self.0.get()
    }

    /// Publish result availability after updating the provider's queue.
    ///
    /// A true update wakes registered waiters after publishing the observation. No provider
    /// callback is invoked and the operation does not sleep. Serialize competing updates
    /// with the actual result queue so that an older observation cannot replace a newer one.
    pub fn update(&self, ready: bool) {
        // SAFETY: The native atomic update and notification borrow this retained object.
        unsafe { bindings::drm_capture_readiness_update(self.as_raw(), ready) };
    }

    /// Observe availability without acknowledging, reserving or authorizing a result.
    pub fn has_results(&self) -> bool {
        // SAFETY: The native operation atomically reads this retained object's flag.
        unsafe { bindings::drm_capture_readiness_has_results(self.as_raw()) }
    }
}

#[cfg(CONFIG_KUNIT)]
#[kunit_tests(rust_drm_capture_readiness)]
mod tests {
    use super::*;

    #[test]
    fn retained_references_share_the_same_observation() -> Result {
        let readiness = Readiness::new()?;
        let retained = readiness.clone();
        assert!(!retained.has_results());
        readiness.update(true);
        drop(readiness);
        assert!(retained.has_results());
        retained.update(false);
        assert!(!retained.has_results());
        Ok(())
    }

    #[test]
    fn independent_objects_do_not_share_availability() -> Result {
        let first = Readiness::new()?;
        let second = Readiness::new()?;
        first.update(true);
        assert!(first.has_results());
        assert!(!second.has_results());
        second.update(true);
        first.update(false);
        assert!(!first.has_results());
        assert!(second.has_results());
        Ok(())
    }
}
