// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Task-local modeset lock ownership for transactions and driver control.

use super::{
    crtc::{
        Crtc,
        DriverCrtc, //
    },
    preparation,
    KmsDriver,
    ModeObject, //
};
use crate::{
    bindings,
    drm::{
        device::{
            Device,
            Registered, //
        },
        preparation::Source, //
    },
    error::to_result,
    prelude::*,
    types::{
        NotThreadSafe,
        Opaque, //
    }, //
};

// Intrusive context lists must stay pinned on the initializing task until all users leave.
#[pin_data(PinnedDrop)]
pub(super) struct ModesetAcquireContext {
    #[pin]
    raw: Opaque<bindings::drm_modeset_acquire_ctx>,
    _task: NotThreadSafe,
}

impl ModesetAcquireContext {
    pub(super) fn new(flags: u32) -> impl PinInit<Self> {
        pin_init!(Self {
            raw <- Opaque::ffi_init(|slot| {
                // SAFETY: The slot is pinned, writable storage for the acquire context.
                unsafe { bindings::drm_modeset_acquire_init(slot, flags) };
            }),
            _task: NotThreadSafe,
        })
    }

    pub(super) fn as_raw(&self) -> *mut bindings::drm_modeset_acquire_ctx {
        self.raw.get()
    }
}

#[pinned_drop]
impl PinnedDrop for ModesetAcquireContext {
    fn drop(self: Pin<&mut Self>) {
        // SAFETY: The task-local owner outlives all callbacks and temporary states using
        // its context. Native cleanup releases any acquired subset, including after errors.
        unsafe {
            bindings::drm_modeset_drop_locks(self.raw.get());
            bindings::drm_modeset_acquire_fini(self.raw.get());
        }
    }
}

/// Callback-local access while one device's connection, CRTC and plane locks are held.
///
/// No lock or borrowed state escapes the callback. The view does not represent a transaction,
/// permission to read pixels, completed presentation or continuing control after return.
pub struct LockedState<'a, T: KmsDriver> {
    device: &'a Device<T>,
    _task: NotThreadSafe,
}

impl<T: KmsDriver> LockedState<'_, T> {
    pub(super) fn device(&self) -> &Device<T> {
        self.device
    }

    /// Observe the accepted source for a CRTC on this device, without retaining pixel access.
    ///
    /// The source identifies accepted state, not completion of its commit tail. A control
    /// operation may compare it with a separately stabilized driver publication.
    pub fn preparation_source<'a, C: DriverCrtc<Driver = T>>(
        &'a self,
        crtc: &'a Crtc<C>,
    ) -> Result<Option<&'a Source>> {
        if self.device.as_raw() != crtc.drm_dev().as_raw() {
            return Err(EINVAL);
        }
        // SAFETY: The view retains initialized state and all modeset locks on this device.
        // The result is borrowed only while that view remains borrowed inside its callback.
        Ok(unsafe { preparation::current_source(crtc) })
    }
}

impl<T: KmsDriver> Device<T, Registered> {
    /// Run driver control once with this device's modeset state stabilized.
    ///
    /// Lock acquisition is interruptible and backs off internally on contention before
    /// calling `control`. The callback runs exactly once on successful acquisition, never
    /// as part of a retry. Its result is returned unchanged after releasing the locks.
    ///
    /// Call without modeset locks held. The callback must not acquire more modeset locks,
    /// submit transactions, wait for commit tails or rendering, or release final DRM
    /// references. It may acquire inner driver locks in their documented order. No other
    /// device is locked; this does not stabilize a separate native rendering GPU.
    pub fn with_modeset_locks<R>(
        &self,
        control: impl FnOnce(&LockedState<'_, T>) -> R,
    ) -> Result<R> {
        // SAFETY: Registration establishes initialized mode objects and excludes teardown.
        unsafe { with_locked_state(self, control) }
    }
}

/// Run control on an initialized device, also used by the private runtime consumer.
///
/// # Safety
///
/// Mode configuration and static object creation must be complete. The caller excludes
/// teardown and further static object creation for the entire call. The callback has the
/// locking obligations of `Device::with_modeset_locks`.
pub(super) unsafe fn with_locked_state<T: KmsDriver, R>(
    device: &Device<T>,
    control: impl FnOnce(&LockedState<'_, T>) -> R,
) -> Result<R> {
    pin_init::stack_pin_init!(let ctx = ModesetAcquireContext::new(
        bindings::DRM_MODESET_ACQUIRE_INTERRUPTIBLE,
    ));
    loop {
        // SAFETY: Registration excludes teardown. The pinned context belongs to this
        // task and is reused only after native backoff on its actual contended lock.
        let result = unsafe { bindings::drm_modeset_lock_all_ctx(device.as_raw(), ctx.as_raw()) };
        if result == EDEADLK.to_errno() {
            // SAFETY: EDEADLK denotes native contention in this initialized context.
            to_result(unsafe { bindings::drm_modeset_backoff(ctx.as_raw()) })?;
        } else {
            to_result(result)?;
            break;
        }
    }
    Ok(control(&LockedState {
        device,
        _task: NotThreadSafe,
    }))
}
