// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Owned destination access detached from capture file and queue lifetimes.

use crate::{
    error::to_result,
    prelude::*, //
};
use core::{
    ffi::c_void,
    marker::PhantomData, //
};

/// Destination work that owns every reference needed until its access ends.
///
/// Submission transfers the entire value; no client or compositor-source borrow may
/// escape into it. Completion publication and cancellation belong to the implementation.
/// Blocking exporter operations must not hold a client queue or source-read claim.
///
/// # Safety
///
/// `OwnerModule` must retain the callback, destructor and their dependencies until run
/// returns. The vtable macro selects the local module. Native dispatch releases that
/// module reference only after the callback and payload destructor have returned.
#[vtable]
pub unsafe trait Delivery: Send + 'static {
    /// Perform the owned operation, including any necessary completion publication.
    fn run(self);
}

struct Callbacks<T>(PhantomData<T>);

impl<T: Delivery> Callbacks<T> {
    const OPS: bindings::drm_capture_delivery_ops = bindings::drm_capture_delivery_ops {
        owner: crate::module::this_module::<T::OwnerModule>().as_ptr(),
        run: Some(Self::run),
    };

    unsafe extern "C" fn run(data: *mut c_void) {
        // SAFETY: Successful submission transfers one initialized KBox<T> to native work.
        // Dispatch invokes this callback exactly once while retaining T's module.
        let delivery = unsafe { KBox::from_raw(data.cast::<T>()) };
        T::run(KBox::into_inner(delivery));
    }
}

/// Submit one owned delivery to the bounded, dedicated native queue.
///
/// Failure drops the supplied value without invoking run. Success keeps both the value
/// and its module alive independently of the submitting client. No completion, bounded
/// exporter latency or storage-reuse guarantee follows merely from successful submission.
pub fn submit<T: Delivery>(delivery: T) -> Result {
    let raw = KBox::into_raw(KBox::new(delivery, GFP_KERNEL)?);
    // SAFETY: The callback table has static lifetime and its module covers T. On success
    // native work owns the initialized allocation; on error it neither retains nor uses it.
    let result = to_result(unsafe {
        bindings::drm_capture_delivery_submit(&Callbacks::<T>::OPS, raw.cast())
    });
    if result.is_err() {
        // SAFETY: Rejected submission leaves the original KBox ownership with this caller.
        drop(unsafe { KBox::from_raw(raw) });
    }
    result
}

#[cfg(CONFIG_KUNIT)]
mod tests;
