// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Owned requests for kernel-controlled final-image capture.
//!
//! These wrappers manage private CPU result storage, not DRM pixel permission or GPU submission.
//! Providers must establish the source and recipient policy before using a stream. Every operation,
//! including destructors, requires a context that may sleep.

use crate::{
    error::from_err_ptr,
    prelude::*,
    sync::aref::{ARef, AlwaysRefCounted},
    types::Opaque,
};
use core::ptr::NonNull;

/// A fixed-size stream of already-authorized final images.
///
/// Dropping a reference does not shut down other owners. Call [`Self::shutdown`] to stop delivery.
///
/// # Invariants
///
/// The underlying C stream is initialized and remains live for every reference to this type.
#[repr(transparent)]
pub struct Stream(Opaque<bindings::drm_capture>);

// SAFETY: The C core serializes stream operations and reference counting is thread-safe.
unsafe impl Send for Stream {}
// SAFETY: Shared access only exposes operations serialized by the C core.
unsafe impl Sync for Stream {}

// SAFETY: Stream lifetime is governed by the C core's get/put reference count.
unsafe impl AlwaysRefCounted for Stream {
    fn inc_ref(&self) {
        // SAFETY: A live shared reference guarantees a nonzero reference count.
        unsafe { bindings::drm_capture_get(self.0.get()) };
    }

    unsafe fn dec_ref(ptr: NonNull<Self>) {
        // SAFETY: The caller transfers a live reference; Stream has the C object's layout.
        unsafe { bindings::drm_capture_put(ptr.as_ptr().cast()) };
    }
}

impl Stream {
    /// Allocate a bounded stream after the provider has authorized its scope and recipient.
    pub fn new(capacity: u32, frame_size: usize) -> Result<ARef<Self>> {
        // SAFETY: The constructor accepts scalar limits and returns ownership or an error.
        let ptr = from_err_ptr(unsafe { bindings::drm_capture_create(capacity, frame_size) })?;
        // SAFETY: Successful creation returns a non-null initialized stream with one reference.
        Ok(unsafe { ARef::from_raw(NonNull::new_unchecked(ptr.cast())) })
    }

    /// Stop admission and discard delivery, leaving active provider ownership intact.
    pub fn shutdown(&self) {
        // SAFETY: The stream is live; shutdown is serialized and idempotent.
        unsafe { bindings::drm_capture_shutdown(self.0.get()) };
    }

    /// Revoke permission without ending active provider access early.
    pub fn revoke(&self) {
        // SAFETY: The stream is live and the C core serializes revocation.
        unsafe { bindings::drm_capture_revoke(self.0.get()) };
    }
}
