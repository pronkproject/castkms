// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Provider-scoped source admission serialization.

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

/// Shared admission accounting for related sources, independent of pixel authority.
///
/// Sources retain the domain even after the creator drops its reference. All operations,
/// including final reference release, require a context that may sleep.
///
/// # Invariants
///
/// Every reference retains an initialized native domain with a live reference count.
#[repr(transparent)]
pub struct Domain(pub(super) Opaque<bindings::drm_prepare_domain>);

// SAFETY: Native domain reference counting is thread-safe and its mutex serializes accounting.
unsafe impl Send for Domain {}
// SAFETY: Shared access exposes only synchronized native operations.
unsafe impl Sync for Domain {}

// SAFETY: Native get/put maintain the initialized domain allocation.
unsafe impl AlwaysRefCounted for Domain {
    fn inc_ref(&self) {
        // SAFETY: The shared reference proves the domain remains initialized and live.
        unsafe { bindings::drm_prepare_domain_get(self.0.get()) };
    }

    unsafe fn dec_ref(ptr: NonNull<Self>) {
        // SAFETY: The caller transfers a reference to the identically represented domain.
        unsafe { bindings::drm_prepare_domain_put(ptr.as_ptr().cast()) };
    }
}

impl Domain {
    /// Allocate a domain for sources that may be prepared together.
    pub fn new() -> Result<ARef<Self>> {
        // SAFETY: Creation returns an owned initialized domain or an error pointer.
        let raw = from_err_ptr(unsafe { bindings::drm_prepare_domain_create() })?;
        // SAFETY: Successful creation transfers a non-null native reference.
        Ok(unsafe { ARef::from_raw(NonNull::new_unchecked(raw.cast())) })
    }
}

#[cfg(CONFIG_KUNIT)]
mod tests;
