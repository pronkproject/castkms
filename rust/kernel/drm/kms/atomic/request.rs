// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Rebuildable kernel requests with native source preparation.

use super::{AtomicStateComposer, Device, KmsDriver};
use crate::{bindings, error::to_result, prelude::*};
use core::{ffi::c_void, mem::ManuallyDrop, ptr::NonNull};

// The native runner supplies a fresh, exclusively borrowed transaction for T and calls the
// builder synchronously. Its context owns the transaction and all acquired modeset locks.
unsafe fn build<T: KmsDriver>(
    raw: *mut bindings::drm_atomic_commit,
    update: &mut impl FnMut(Pin<&mut AtomicStateComposer<T>>) -> Result,
) -> i32 {
    // SAFETY: The native runner owns this non-null unpublished transaction for the callback.
    // Suppressing drop borrows its reference instead of consuming the runner's ownership.
    let mut state =
        ManuallyDrop::new(unsafe { AtomicStateComposer::<T>::new(NonNull::new_unchecked(raw)) });
    // SAFETY: The stack-local composer does not move again and cannot escape the callback.
    let result = update(unsafe { Pin::new_unchecked(&mut *state) });
    // A builder which consumes a lock error must not allow a partial attempt to be accepted.
    // SAFETY: The runner owns the initialized acquire context throughout this invocation.
    if unsafe { !(*(*raw).acquire_ctx).contended.is_null() } {
        EDEADLK.to_errno()
    } else {
        result.err().map_or(0, Error::to_errno)
    }
}

unsafe extern "C" fn build_callback<T, F>(
    raw: *mut bindings::drm_atomic_commit,
    data: *mut c_void,
) -> i32
where
    T: KmsDriver,
    F: FnMut(Pin<&mut AtomicStateComposer<T>>) -> Result,
{
    // SAFETY: run supplies exclusive access to F for synchronous callbacks on fresh state.
    unsafe { build::<T>(raw, &mut *data.cast::<F>()) }
}

// The caller excludes setup, registration and teardown of the initialized KMS device.
pub(super) unsafe fn run<T, F>(dev: &Device<T>, mut update: F) -> Result
where
    T: KmsDriver,
    F: FnMut(Pin<&mut AtomicStateComposer<T>>) -> Result,
{
    // SAFETY: The device remains initialized, and update is exclusively borrowed until the
    // synchronous runner returns. Neither its data pointer nor borrowed states are retained.
    to_result(unsafe {
        bindings::drm_atomic_commit_request(
            dev.as_raw(),
            Some(build_callback::<T, F>),
            (&mut update as *mut F).cast(),
        )
    })
}

#[cfg(CONFIG_KUNIT)]
struct Checked<F, H> {
    update: F,
    after_check: H,
}

#[cfg(CONFIG_KUNIT)]
impl<F, H> Checked<F, H> {
    unsafe extern "C" fn build<T: KmsDriver>(
        raw: *mut bindings::drm_atomic_commit,
        data: *mut c_void,
    ) -> i32
    where
        F: FnMut(Pin<&mut AtomicStateComposer<T>>) -> Result,
    {
        // SAFETY: run_after_check provides exclusive access to this live context. Native
        // callbacks are synchronous, serial, and supply an unpublished transaction for T.
        unsafe { build::<T>(raw, &mut (*data.cast::<Self>()).update) }
    }

    unsafe extern "C" fn prepare(_raw: *mut bindings::drm_atomic_commit, data: *mut c_void) -> i32
    where
        H: FnMut() -> Result,
    {
        // SAFETY: run_after_check retains exclusive ownership until all callbacks finish.
        let request = unsafe { &mut *data.cast::<Self>() };
        (request.after_check)().err().map_or(0, Error::to_errno)
    }

    unsafe extern "C" fn complete(
        _raw: *mut bindings::drm_atomic_commit,
        _accepted: bool,
        _data: *mut c_void,
    ) {
        // The injection hook owns no per-attempt signaling resources.
    }
}

// The caller supplies the same initialized-device and exclusion guarantees as run.
#[cfg(CONFIG_KUNIT)]
pub(super) unsafe fn run_after_check<T, F, H>(dev: &Device<T>, update: F, after_check: H) -> Result
where
    T: KmsDriver,
    F: FnMut(Pin<&mut AtomicStateComposer<T>>) -> Result,
    H: FnMut() -> Result,
{
    let mut request = Checked {
        update,
        after_check,
    };
    let callbacks = bindings::drm_atomic_request_callbacks {
        build: Some(Checked::<F, H>::build::<T>),
        prepare_signaling: Some(Checked::<F, H>::prepare),
        complete_signaling: Some(Checked::<F, H>::complete),
    };
    // SAFETY: The initialized device, callback table and exclusively borrowed context remain
    // live until every native callback completes. No state or context pointer escapes.
    to_result(unsafe {
        bindings::drm_atomic_commit_request_with_callbacks(
            dev.as_raw(),
            core::ptr::null_mut(),
            &callbacks,
            (&mut request as *mut Checked<F, H>).cast(),
        )
    })
}
