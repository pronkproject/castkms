// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Connect KMS setup and accepted output state to shared preparation accounting.

use super::{
    crtc::{
        AsRawCrtc,
        AsRawCrtcStatePrivate,
        Crtc,
        CrtcAtomicCommit,
        DriverCrtc, //
    },
    KmsDriver,
    ModeObject,
    UnregisteredKmsDevice, //
};
use crate::{
    drm::{
        device::{
            Device,
            Registered, //
        },
        preparation::Source, //
    },
    error::to_result,
    prelude::*,
    sync::aref::ARef, //
};
use core::ptr::NonNull;

impl<T: KmsDriver> Device<T, Registered> {
    /// Observe the accepted CRTC generation while excluding another atomic state swap.
    ///
    /// The callback runs exactly once after acquiring the CRTC's modeset lock, or not at all
    /// if locking is interrupted or the CRTC belongs to another device. It must not acquire
    /// other modeset locks, submit transactions, wait for a commit tail, or release final DRM
    /// references. Call without any modeset locks held.
    ///
    /// `None` denotes that no preparation source has been initialized. A source identifies
    /// accepted state, not completion of its commit tail or installation of a driver's scene.
    /// The callback may compare it with a separately locked publication before changing driver
    /// control state.
    /// Neither observation nor retaining the source grants access to pixels.
    pub fn with_crtc_preparation_source<C: DriverCrtc<Driver = T>, R>(
        &self,
        crtc: &Crtc<C>,
        observe: impl FnOnce(Option<&Source>) -> R,
    ) -> Result<R> {
        if self.as_raw() != crtc.drm_dev().as_raw() {
            return Err(EINVAL);
        }
        // SAFETY: Registration excludes teardown and the identity check establishes that the
        // CRTC belongs to this initialized device.
        unsafe { with_current_source(crtc, observe) }
    }
}

/// Observe one initialized CRTC, also used by the unregistered runtime consumer.
///
/// # Safety
///
/// CRTC setup, including its initial state, must be complete. The caller must exclude
/// teardown and object creation for the call. The callback has the locking obligations of
/// `Device::with_crtc_preparation_source`.
pub(super) unsafe fn with_current_source<C: DriverCrtc, R>(
    crtc: &Crtc<C>,
    observe: impl FnOnce(Option<&Source>) -> R,
) -> Result<R> {
    // SAFETY: The caller retains the initialized CRTC and excludes cleanup of its mutex.
    let lock = unsafe { &raw mut (*crtc.as_raw()).mutex };
    // SAFETY: No acquire context is needed for this single, non-nested modeset lock.
    to_result(unsafe { bindings::drm_modeset_lock_single_interruptible(lock) })?;
    struct Unlock(*mut bindings::drm_modeset_lock);
    impl Drop for Unlock {
        fn drop(&mut self) {
            // SAFETY: The owner is created only after successful acquisition on this task,
            // remains local to the call and is dropped before the CRTC borrow ends.
            unsafe { bindings::drm_modeset_unlock(self.0) };
        }
    }
    let _unlock = Unlock(lock);
    // SAFETY: The modeset lock stabilizes the source and teardown is excluded by the caller.
    let source = unsafe { current_source(crtc) };
    Ok(observe(source))
}

/// Borrow initialized CRTC state while its modeset lock is held.
///
/// # Safety
///
/// The caller must exclude teardown and hold the CRTC's modeset lock for every use of
/// the returned reference, not merely this function call. No pixel access is granted.
pub(super) unsafe fn current_source<C: DriverCrtc>(crtc: &Crtc<C>) -> Option<&Source> {
    // SAFETY: The caller stabilizes the initialized state and its retained source pointer.
    unsafe {
        let state = (*crtc.as_raw()).state;
        if state.is_null() {
            None
        } else {
            NonNull::new((*state).prepare_source).map(|source| &*source.cast::<Source>().as_ptr())
        }
    }
}

impl<T: KmsDriver> UnregisteredKmsDevice<'_, T> {
    /// Enable preparation before creating CRTCs during single-threaded setup.
    ///
    /// `capacity` limits admitted reads per output generation, independently of media queues.
    /// The Rust KMS implementation uses the shared state lifetime and atomic commit helpers.
    /// Enabling accounting does not authorize capture or publish source buffers.
    pub fn enable_preparation(&self, capacity: u32) -> Result {
        // SAFETY: The setup view retains initialized mode configuration and excludes concurrent
        // registration or object creation. Native initialization rejects zero capacity, repeated
        // setup and setup after CRTC creation; mode configuration cleanup releases its resources.
        to_result(unsafe { bindings::drm_atomic_prepare_display_init(self.as_raw(), capacity) })
    }
}

impl<T: DriverCrtc> CrtcAtomicCommit<'_, T> {
    /// Retain read accounting for the output generation accepted by this commit.
    ///
    /// Returns `None` when the device has not enabled preparation. Accounting also exists for
    /// disabled outputs; the reference does not imply that the output has a visible image.
    /// It grants neither capture authority nor pixel access. A provider must authorize a read,
    /// retain its storage, and acquire a claim separately before accessing source pixels.
    pub fn preparation_source(&self) -> Option<ARef<Source>> {
        let (_, new) = self.old_new_state();
        // SAFETY: The callback retains the accepted state. Preparation initializes its source
        // before publication and does not replace the pointer while the state remains live.
        let source = NonNull::new(unsafe { (*new.as_raw()).prepare_source })?;
        // SAFETY: The accepted state retains the initialized source while we acquire a reference.
        unsafe { bindings::drm_prepare_source_get(source.as_ptr()) };
        // SAFETY: Source transparently represents the native object. Transfer the acquired
        // reference to its Rust owner, independently of the callback and accepted state.
        Some(unsafe { ARef::from_raw(source.cast()) })
    }
}
