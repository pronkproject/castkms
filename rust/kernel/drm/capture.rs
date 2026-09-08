// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Owned requests for kernel-controlled final-image capture.
//!
//! These wrappers manage private CPU result storage, not DRM pixel permission or GPU submission.
//! Providers must establish the source and recipient policy before using a stream. Every operation,
//! including destructors, requires a context that may sleep.

use crate::{
    error::{from_err_ptr, to_result},
    prelude::*,
    sync::aref::{ARef, AlwaysRefCounted},
    types::Opaque,
};
use core::{mem::ManuallyDrop, ptr::NonNull};

#[cfg(CONFIG_KUNIT)]
mod tests;

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

    /// Queue a request whose identity and cleanup remain tied to this stream.
    pub fn queue(&self) -> Result<Request> {
        let mut id = 0;
        // SAFETY: The stream is live and id is writable for the duration of the call.
        to_result(unsafe { bindings::drm_capture_queue(self.0.get(), &mut id) })?;
        Ok(Request {
            stream: self.into(),
            id,
        })
    }

    /// Claim the oldest request for synchronous, kernel-controlled pixel production.
    ///
    /// The provider must validate current source permission before claiming. This operation does
    /// not perform authority registration checks; authority-managed providers must claim through
    /// their authority instead. No source or asynchronous device access is represented by [`Job`].
    pub fn claim(&self) -> Result<Job> {
        // SAFETY: A live stream is sufficient for claiming; the returned job is exclusively owned.
        let ptr = from_err_ptr(unsafe { bindings::drm_capture_claim(self.0.get()) })?;
        // SAFETY: A successful claim returns a non-null job with independent storage ownership.
        Ok(Job {
            ptr: unsafe { NonNull::new_unchecked(ptr) },
        })
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

/// One request in its originating stream; dropping it abandons demand or the retained result.
///
/// If a provider has claimed the request, dropping it preserves the provider's storage and credit
/// until completion. There is no transferable numeric identifier and no implicit stream shutdown.
#[must_use = "dropping a request abandons its capture result"]
pub struct Request {
    stream: ARef<Stream>,
    id: u64,
}

/// Completion state; a completed access may still have failed to produce a valid image.
#[derive(Debug, PartialEq, Eq)]
pub enum Status {
    /// Provider work is outstanding, or the request has not been claimed.
    Pending,
    /// Access ended; success permits copying the image, an error does not.
    Complete(Result),
}

impl Request {
    /// Inspect completion without consuming the result.
    pub fn status(&self) -> Result<Status> {
        let mut result = bindings::drm_capture_result::default();
        // SAFETY: This request retains its stream and the output is writable until return.
        to_result(unsafe {
            bindings::drm_capture_query(self.stream.0.get(), self.id, &mut result)
        })?;
        Ok(if result.completed {
            Status::Complete(to_result(result.status))
        } else {
            Status::Pending
        })
    }

    /// Copy a successful result; pending or failed requests leave the buffer unchanged.
    pub fn copy_result(&self, output: &mut [u8]) -> Result<usize> {
        // SAFETY: The stream is retained and the slice describes all writable output storage.
        let ret = unsafe {
            bindings::drm_capture_copy_result(
                self.stream.0.get(),
                self.id,
                output.as_mut_ptr().cast(),
                output.len(),
            )
        };
        if ret < 0 {
            // C returns a negative errno, which fits i32.
            Err(Error::from_errno(ret as i32))
        } else {
            Ok(ret as usize)
        }
    }

    /// Cancel delivery but retain the eventual terminal status for inspection.
    pub fn cancel(&self) -> Result {
        // SAFETY: The retained stream and stream-local identifier are valid for the call.
        to_result(unsafe { bindings::drm_capture_cancel(self.stream.0.get(), self.id) })
    }
}

impl Drop for Request {
    fn drop(&mut self) {
        // SAFETY: The stream remains live through this destructor. A prior shutdown may already
        // have discarded the request; ignoring ENOENT is intentional. Active job storage survives.
        unsafe { bindings::drm_capture_discard(self.stream.0.get(), self.id) };
    }
}

/// Exclusive access to private CPU result storage, completed exactly once or canceled on drop.
///
/// Only synchronous borrows of the image are exposed. This type must not be used to represent
/// outstanding GPU or other asynchronous access. Forgetting it leaks ownership, never frees early.
///
/// # Invariants
///
/// `ptr` owns one claimed C job and its completion obligation, until consumed or dropped.
#[must_use = "dropping a job completes it with ECANCELED"]
pub struct Job {
    ptr: NonNull<bindings::drm_capture_job>,
}

// SAFETY: Exclusive job ownership and its independently retained storage may move between threads.
unsafe impl Send for Job {}

impl Job {
    /// Borrow the initialized image bytes exclusively until the borrow ends.
    pub fn data_mut(&mut self) -> &mut [u8] {
        // SAFETY: A claimed job retains its allocation despite cancellation, discard or shutdown.
        let data = unsafe { bindings::drm_capture_job_data(self.ptr.as_ptr()) };
        // SAFETY: The job retains the stream containing its immutable, nonzero image size.
        let size = unsafe { bindings::drm_capture_job_size(self.ptr.as_ptr()) };
        // SAFETY: The C core allocated and zeroed `size` bytes, bounded by SSIZE_MAX. Exclusive job
        // ownership and &mut self prevent other borrows; consumers cannot copy a pending result.
        unsafe { core::slice::from_raw_parts_mut(data.cast(), size) }
    }

    /// End CPU access and consume the completion obligation; revocation may still deny delivery.
    pub fn complete(self, status: Result) {
        let job = ManuallyDrop::new(self);
        // SAFETY: Consuming self ends every image borrow and transfers its sole job ownership.
        unsafe {
            bindings::drm_capture_complete(
                job.ptr.as_ptr(),
                status.err().map_or(0, Error::to_errno),
            )
        };
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: All synchronous borrows ended before drop and this owns the completion obligation.
        unsafe { bindings::drm_capture_complete(self.ptr.as_ptr(), ECANCELED.to_errno()) };
    }
}
