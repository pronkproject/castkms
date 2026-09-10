// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Private, unregistered devices for testing driver atomic callbacks.
//!
//! No DRM minor is published. These fixtures exercise kernel transactions, not file ioctl
//! authorization. Drivers must release any device-owned framebuffer references before dropping
//! the fixture, just as their registration owner must do during shutdown.

use super::{
    atomic::{
        self,
        AtomicStateComposer, //
    },
    connector::{
        AsRawConnector,
        Connector, //
    },
    crtc::{
        AsRawCrtc,
        Crtc, //
    },
    framebuffer::{
        Framebuffer,
        FramebufferLayout,
        FramebufferRef, //
    },
    plane::{
        AsRawPlane,
        Plane, //
    },
    private::KmsImpl,
    KmsDriver, //
};
use crate::{
    bindings,
    drm::{
        auth::{
            MasterRef,
            MasterSnapshot, //
        },
        Device,
        UnregisteredDevice, //
    },
    prelude::*, //
};
use core::ptr::NonNull;

#[cfg(CONFIG_DRM_CLIENT)]
mod buffers;

// Native master allocation stays in the built-in test support, not in driver modules.
#[inline(never)]
fn allocate_master(dev: *mut bindings::drm_device) -> Result<NonNull<bindings::drm_master>> {
    // SAFETY: The private caller retains its initialized DRM device across this call.
    NonNull::new(unsafe { bindings::drm_master_create(dev) }).ok_or(ENOMEM)
}

/// Initialized KMS configuration kept outside userspace registration.
///
/// Consuming the unregistered device prevents registration or further object construction
/// through its setup view. Object borrows remain tied to this owner.
pub struct TestDevice<T: KmsDriver>(UnregisteredDevice<T>);

impl<T: KmsDriver> Drop for TestDevice<T> {
    fn drop(&mut self) {
        // SAFETY: The fixture owns completed KMS setup and excludes concurrent transactions.
        // Retire accepted state before releasing the device's owning reference.
        unsafe { bindings::drm_atomic_helper_shutdown(self.0.as_raw()) };
    }
}

impl<T: KmsDriver> TestDevice<T> {
    /// Initialize the driver's actual KMS objects and initial states without publishing them.
    pub fn new(dev: UnregisteredDevice<T>) -> Result<Self> {
        // SAFETY: Consuming the newly allocated device excludes registration. The only setup
        // entry point for external callers consumes that owner as well.
        unsafe { <T as KmsImpl>::setup_kms(&dev) }?;
        Ok(Self(dev))
    }

    /// Borrow the initialized device for allocation and driver-private observations.
    pub fn device(&self) -> &Device<T> {
        &self.0
    }

    /// Borrow the only CRTC, rejecting fixtures with a different topology.
    pub fn crtc(&self) -> Result<&Crtc<T::Crtc>> {
        // SAFETY: Setup is complete, and this unregistered fixture excludes topology changes
        // and teardown. Rust constructors enforce the nominated concrete object type.
        unsafe {
            let config = &raw const (*self.0.as_raw()).mode_config;
            if (*config).num_crtc != 1 {
                return Err(EINVAL);
            }
            let raw = crate::container_of!((*config).crtc_list.next, bindings::drm_crtc, head);
            Ok(Crtc::from_raw(raw))
        }
    }

    /// Borrow the only plane, rejecting fixtures with a different topology.
    pub fn plane(&self) -> Result<&Plane<T::Plane>> {
        // SAFETY: Completed, privately owned setup excludes topology changes and teardown.
        // Rust constructors enforce the driver's nominated concrete plane type.
        unsafe {
            let config = &raw const (*self.0.as_raw()).mode_config;
            if (*config).num_total_plane != 1 {
                return Err(EINVAL);
            }
            let raw = crate::container_of!((*config).plane_list.next, bindings::drm_plane, head);
            Ok(Plane::from_raw(raw))
        }
    }

    /// Borrow the only connector, rejecting fixtures with a different topology.
    pub fn connector(&self) -> Result<&Connector<T::Connector>> {
        // SAFETY: The same completed, exclusively owned topology guarantee as crtc() applies.
        unsafe {
            let config = &raw const (*self.0.as_raw()).mode_config;
            if (*config).num_connector != 1 {
                return Err(EINVAL);
            }
            let raw =
                crate::container_of!((*config).connector_list.next, bindings::drm_connector, head);
            Ok(Connector::from_raw(raw))
        }
    }

    /// Construct a framebuffer using the shared layout checks and driver metadata storage.
    pub fn framebuffer(
        &self,
        layout: &FramebufferLayout<'_, T>,
        data: T::FramebufferData,
    ) -> Result<FramebufferRef<T>> {
        // SAFETY: This owner protects completed KMS setup. The returned framebuffer retains
        // its device; driver-held references must be released before fixture shutdown.
        unsafe { Framebuffer::from_objects_with_data_unchecked(&self.0, layout, data) }
    }

    /// Insert a native dependency into a framebuffer owned by this private test device.
    ///
    /// The caller must not hold a reservation lock. The framebuffer is retained throughout
    /// insertion, and the native reservation acquires its own fence reference.
    pub fn add_framebuffer_fence(
        &self,
        framebuffer: &Framebuffer<T>,
        plane: usize,
        fence: &crate::dma_fence::Fence,
        usage: crate::dma_resv::Usage,
    ) -> Result {
        use super::ModeObject;
        use crate::drm::gem::IntoGEMObject;
        use crate::error::to_result;

        if !core::ptr::eq(framebuffer.drm_dev(), self.device()) {
            return Err(EINVAL);
        }
        let object = framebuffer.object_at(plane)?;
        // SAFETY: The framebuffer retains its initialized GEM object and reservation.
        let reservation = unsafe { (*object.as_raw()).resv };
        // SAFETY: A single private reservation is locked without an enclosing acquire context.
        to_result(unsafe { bindings::dma_resv_lock(reservation, core::ptr::null_mut()) })?;
        // SAFETY: Insertion holds the lock and reserves capacity before adding a reference.
        let result = to_result(unsafe { bindings::dma_resv_reserve_fences(reservation, 1) });
        if result.is_ok() {
            unsafe { bindings::dma_resv_add_fence(reservation, fence.as_raw(), usage as _) };
        }
        // SAFETY: Balance the lock on both allocation outcomes.
        unsafe { bindings::dma_resv_unlock(reservation) };
        result
    }

    /// Submit a blocking transaction through the installed driver callbacks.
    ///
    /// The callback must propagate errors and be replayable after lock contention. Do not
    /// hold modeset locks or recursively submit transactions from the callback.
    pub fn update(&self, update: impl FnMut(Pin<&mut AtomicStateComposer<T>>) -> Result) -> Result {
        // SAFETY: The fixture owns initialized mode configuration and excludes teardown.
        unsafe { atomic::run_update(&self.0, update) }
    }

    /// Validate without publishing state or invoking commit callbacks.
    ///
    /// The callback has the same replay and locking requirements as [`Self::update`].
    pub fn check(&self, update: impl FnMut(Pin<&mut AtomicStateComposer<T>>) -> Result) -> Result {
        // SAFETY: The same initialized-device guarantee as update() applies.
        unsafe { atomic::run_check(&self.0, update) }
    }

    /// Create synthetic creation evidence backed by a real retained native master identity.
    ///
    /// No file becomes master and no master callback is invoked. `was_current` is test input,
    /// not an observation of native authority. File-transition tests are needed separately.
    pub fn synthetic_master_snapshot(&self, was_current: bool) -> Result<MasterSnapshot<T>> {
        let raw = allocate_master(self.0.as_raw())?;
        // SAFETY: The constructor returned one owned reference belonging to this device.
        let master = unsafe { MasterRef::from_owned_raw(raw, &self.0) };
        Ok(MasterSnapshot::new(master, was_current))
    }
}
