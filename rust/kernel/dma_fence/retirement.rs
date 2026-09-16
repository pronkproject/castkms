// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Preallocated cleanup owners whose payloads outlive submitted native access.

use super::Fence;
use crate::{error::from_err_ptr, prelude::*};
use core::{ffi::c_void, marker::PhantomData, ptr::NonNull};

/// Owned cleanup performed after native access ends, irrespective of pixel validity.
///
/// The callback runs in sleepable context and must not wait for unrelated future work.
/// It may release storage and update bookkeeping, but grants no authority for new access.
///
/// # Safety
///
/// `OwnerModule` must retain the callback, destructor and all their dependencies until
/// release returns. The vtable macro selects the local module. Native code releases its
/// module reference only after the callback and payload destruction have returned.
#[vtable]
pub unsafe trait Retire: Send + 'static {
    /// Consume the payload after its access has ended, or before any access was admitted.
    fn retire(self);
}

struct Callbacks<T>(PhantomData<T>);

impl<T: Retire> Callbacks<T> {
    const OPS: bindings::dma_fence_retirement_ops = bindings::dma_fence_retirement_ops {
        owner: crate::module::this_module::<T::OwnerModule>().as_ptr(),
        release: Some(Self::release),
    };

    unsafe extern "C" fn release(data: *mut c_void) {
        // SAFETY: Creation transferred one initialized KBox<T>; native ownership invokes
        // release exactly once and retains T's module until payload destruction completes.
        let payload = unsafe { KBox::from_raw(data.cast::<T>()) };
        T::retire(KBox::into_inner(payload));
    }
}

/// Unique cleanup prepared before admitting access to an owned payload.
///
/// Dropping an unsubmitted record releases its payload synchronously. Submission transfers
/// cleanup to a concrete fence without allocation or waiting; neither file closure nor a
/// failed operation releases storage before that fence signals. Callers separately bound
/// outstanding records and must not drop an unsubmitted record while access is uncertain.
pub struct Retirement<T: Retire> {
    raw: NonNull<bindings::dma_fence_retirement>,
    _payload: PhantomData<T>,
}

// SAFETY: The native record is uniquely owned, and its payload may move between tasks.
unsafe impl<T: Retire> Send for Retirement<T> {}

impl<T: Retire> Retirement<T> {
    /// Allocate the cleanup owner before any native access can be submitted.
    /// Failure destroys the supplied payload in the caller's sleepable context.
    pub fn new(payload: T) -> Result<Self> {
        let data = KBox::into_raw(KBox::new(payload, GFP_KERNEL)?);
        // SAFETY: The callback table is static and its owner retains T's implementation.
        // Successful creation transfers the initialized allocation; failure retains none.
        let result = from_err_ptr(unsafe {
            bindings::dma_fence_retirement_create(&Callbacks::<T>::OPS, data.cast())
        });
        match result {
            Ok(raw) => Ok(Self {
                // SAFETY: Successful native creation returns a non-null owned record.
                raw: unsafe { NonNull::new_unchecked(raw) },
                _payload: PhantomData,
            }),
            Err(error) => {
                // SAFETY: Native creation failed without consuming the original allocation.
                drop(unsafe { KBox::from_raw(data) });
                Err(error)
            }
        }
    }

    /// Transfer cleanup to submitted completion covering every use of the payload.
    ///
    /// The caller must close admission of further access before transfer. Native errors
    /// still retire access; the operation's validity is a separate retained result.
    pub fn submit(self, fence: &Fence) {
        let raw = self.raw.as_ptr();
        core::mem::forget(self);
        // SAFETY: The unique unsubmitted record is transferred once. Native code retains
        // the borrowed fence before the call returns and owns every later callback.
        unsafe { bindings::dma_fence_retirement_submit(raw, fence.as_raw()) };
    }
}

impl<T: Retire> Drop for Retirement<T> {
    fn drop(&mut self) {
        // SAFETY: Only unsubmitted records remain owned by Rust. The payload and callback
        // are live until native destruction releases their module reference after return.
        unsafe { bindings::dma_fence_retirement_destroy(self.raw.as_ptr()) };
    }
}

#[cfg(CONFIG_KUNIT)]
mod tests;
