// SPDX-License-Identifier: GPL-2.0 OR MIT

//! DRM driver core.
//!
//! C header: [`include/drm/drm_drv.h`](srctree/include/drm/drm_drv.h)

use crate::{
    bindings,
    device,
    drm::{self, kms::private::KmsImpl as KmsImplPrivate},
    error::to_result,
    prelude::*,
    sync::aref::ARef, //
};
use core::ptr::NonNull;

/// Driver use the GEM memory manager. This should be set for all modern drivers.
pub(crate) const FEAT_GEM: u32 = bindings::drm_driver_feature_DRIVER_GEM;
/// Driver supports render nodes, i.e.: /dev/dri/renderDXX devices.
pub(crate) const FEAT_RENDER: u32 = bindings::drm_driver_feature_DRIVER_RENDER;

/// The driver supports modesetting.
pub(crate) const FEAT_MODESET: u32 = bindings::drm_driver_feature_DRIVER_MODESET;

/// The driver supports atomic modesetting.
pub(crate) const FEAT_ATOMIC: u32 = bindings::drm_driver_feature_DRIVER_ATOMIC;

/// Information data for a DRM Driver.
pub struct DriverInfo {
    /// Driver major version.
    pub major: i32,
    /// Driver minor version.
    pub minor: i32,
    /// Driver patchlevel version.
    pub patchlevel: i32,
    /// Driver name.
    pub name: &'static CStr,
    /// Driver description.
    pub desc: &'static CStr,
}

/// Internal memory management operation set, normally created by memory managers (e.g. GEM).
pub struct AllocOps {
    pub(crate) gem_create_object: Option<
        unsafe extern "C" fn(
            dev: *mut bindings::drm_device,
            size: usize,
        ) -> *mut bindings::drm_gem_object,
    >,
    pub(crate) prime_handle_to_fd: Option<
        unsafe extern "C" fn(
            dev: *mut bindings::drm_device,
            file_priv: *mut bindings::drm_file,
            handle: u32,
            flags: u32,
            prime_fd: *mut core::ffi::c_int,
        ) -> core::ffi::c_int,
    >,
    pub(crate) prime_fd_to_handle: Option<
        unsafe extern "C" fn(
            dev: *mut bindings::drm_device,
            file_priv: *mut bindings::drm_file,
            prime_fd: core::ffi::c_int,
            handle: *mut u32,
        ) -> core::ffi::c_int,
    >,
    pub(crate) gem_prime_import: Option<
        unsafe extern "C" fn(
            dev: *mut bindings::drm_device,
            dma_buf: *mut bindings::dma_buf,
        ) -> *mut bindings::drm_gem_object,
    >,
    pub(crate) gem_prime_import_sg_table: Option<
        unsafe extern "C" fn(
            dev: *mut bindings::drm_device,
            attach: *mut bindings::dma_buf_attachment,
            sgt: *mut bindings::sg_table,
        ) -> *mut bindings::drm_gem_object,
    >,
    pub(crate) dumb_create: Option<
        unsafe extern "C" fn(
            file_priv: *mut bindings::drm_file,
            dev: *mut bindings::drm_device,
            args: *mut bindings::drm_mode_create_dumb,
        ) -> core::ffi::c_int,
    >,
    pub(crate) dumb_map_offset: Option<
        unsafe extern "C" fn(
            file_priv: *mut bindings::drm_file,
            dev: *mut bindings::drm_device,
            handle: u32,
            offset: *mut u64,
        ) -> core::ffi::c_int,
    >,
    pub(crate) fbdev_probe: Option<
        unsafe extern "C" fn(
            fbdev_helper: *mut bindings::drm_fb_helper,
            sizes: *mut bindings::drm_fb_helper_surface_size,
        ) -> core::ffi::c_int,
    >,
}

/// Trait for memory manager implementations. Implemented internally.
pub trait AllocImpl: super::private::Sealed + drm::gem::IntoGEMObject {
    /// The [`Driver`] implementation for this [`AllocImpl`].
    type Driver: drm::Driver;

    /// The C callback operations for this memory manager.
    const ALLOC_OPS: AllocOps;
}

/// The DRM `Driver` trait.
///
/// This trait must be implemented by drivers in order to create a `struct drm_device` and `struct
/// drm_driver` to be registered in the DRM subsystem.
#[vtable]
pub trait Driver {
    /// Context data associated with the DRM driver
    type Data: Sync + Send;

    /// Data owned by the [`Registration`] and accessible within a
    /// [`RegistrationGuard`](drm::RegistrationGuard) critical section via
    /// [`Device::registration_data_with()`](drm::Device::registration_data_with).
    ///
    /// The lifetime parameter is tied to the [`Registration`] scope, which is enclosed in the
    /// parent bus device binding scope but may be shorter.
    type RegistrationData<'a>: Send + Sync + 'a;

    /// The type used to manage memory for this driver.
    type Object: AllocImpl<Driver = Self>;

    /// The type used to represent a DRM File (client)
    type File: drm::file::DriverFile<Driver = Self>;

    /// The bus device type of the parent device that the DRM device is associated with.
    type ParentDevice<Ctx: device::DeviceContext>: device::AsBusDevice<Ctx>;

    /// The KMS implementation for this driver.
    ///
    /// Drivers that wish to support KMS should pass their implementation of `drm::kms::KmsDriver`
    /// here. Drivers which do not have KMS support should use `core::marker::PhantomData<Self>`.
    type Kms: drm::kms::KmsImpl<Driver = Self>
    where
        Self: Sized;

    /// Driver metadata
    const INFO: DriverInfo;

    /// IOCTL list. See `kernel::drm::ioctl::declare_drm_ioctls!{}`.
    const IOCTLS: &'static [drm::ioctl::DrmIoctlDescriptor];

    /// Sets the `DRIVER_RENDER` feature for this driver.
    ///
    /// When enabled, the driver exposes `/dev/dri/renderDXX` render nodes to
    /// userspace. The render node is an alternate low-privilege way to access
    /// the driver, which is enforced on a per-ioctl level. Userspace processes
    /// that open the render node can only invoke ioctls explicitly listed as
    /// usable from the render node (i.e. marked DRM_RENDER_ALLOW), whereas
    /// userspace processes using the master node can invoke any ioctl.
    const FEAT_RENDER: bool = false;

    /// Observe installation or removal of the device's top-level DRM master.
    ///
    /// `Some` retains the installed identity; `None` announces its removal before DRM drops
    /// the device's native reference. Notifications are serialized by DRM's master mutex.
    /// They do not report individual lease changes or grant ongoing authority to a snapshot.
    ///
    /// The callback may precede file initialization or occur after unplug. It must not
    /// acquire DRM's master mutex, perform modesetting, or wait for work needing that mutex.
    /// Device-owned copies must be released on shutdown to avoid retaining the device forever.
    fn master_changed(dev: &drm::Device<Self>, master: Option<drm::auth::MasterRef<Self>>)
    where
        Self: Sized,
    {
        let _ = (dev, master);
    }
}

/// The registration type of a `drm::Device`.
///
/// Once the `Registration` structure is dropped, the device is unregistered.
pub struct Registration<'a, T: Driver> {
    drm: ARef<drm::Device<T>>,
    _reg_data: Pin<KBox<T::RegistrationData<'a>>>,
}

impl<'a, T: Driver> Registration<'a, T> {
    /// Registers a new [`UnregisteredDevice`](drm::UnregisteredDevice) with borrowed
    /// registration data.
    ///
    /// # Safety
    ///
    /// The caller must not `mem::forget()` the returned [`Registration`] or otherwise prevent its
    /// [`Drop`] implementation from running, since the registration data may contain borrowed
    /// references that become invalid after `'a` ends.
    ///
    /// The registration must be dropped before the parent device releases any resources whose
    /// access relies on its bound context, even when registration data is owned. Holding a
    /// reference to the parent allocation does not keep it bound. A devres action alone is not
    /// sufficient: later-added resources may be released before that action runs.
    pub unsafe fn new<E>(
        dev: &device::Device<device::Bound>,
        drm: drm::UnregisteredDevice<T>,
        reg_data: impl PinInit<T::RegistrationData<'a>, E>,
        flags: usize,
    ) -> Result<Self>
    where
        Error: From<E>,
    {
        let parent = drm.as_ref();
        if parent.as_ref().as_raw() != dev.as_raw() {
            return Err(EINVAL);
        }

        let has_kms = drm::Device::<T>::has_kms();
        let mode_config_info = if has_kms {
            // SAFETY: `drm` is still an unregistered device and KMS setup only happens here.
            Some(unsafe { T::Kms::setup_kms(&drm)? })
        } else {
            None
        };

        let reg_data: Pin<KBox<T::RegistrationData<'a>>> = KBox::pin_init(reg_data, GFP_KERNEL)?;

        // Store the registration data pointer in the device before registration, so that it is
        // visible once ioctls can be called.
        let ptr: NonNull<T::RegistrationData<'static>> =
            NonNull::from(Pin::get_ref(reg_data.as_ref())).cast();

        // SAFETY: No concurrent access; the device is not yet registered.
        unsafe { *drm.registration_data.get() = ptr };

        // SAFETY: `drm` is a valid, initialized but not yet registered DRM device.
        let ret = unsafe { bindings::drm_dev_register(drm.as_raw(), flags) };
        if let Err(e) = to_result(ret) {
            // SAFETY: `drm_dev_register()` synchronizes SRCU on failure, so no concurrent
            // access to `registration_data` is possible at this point.
            unsafe { *drm.registration_data.get() = NonNull::dangling() };
            return Err(e);
        }

        #[cfg(CONFIG_DRM_CLIENT)]
        if let Some(info) = mode_config_info
            .as_ref()
            .filter(|info| info.enable_default_client)
        {
            if let Some(fourcc) = info.preferred_fourcc {
                // SAFETY: The DRM device was successfully registered above.
                unsafe { bindings::drm_client_setup_with_fourcc(drm.as_raw(), fourcc) }
            } else {
                // SAFETY: The DRM device was successfully registered above.
                unsafe { bindings::drm_client_setup(drm.as_raw(), core::ptr::null()) }
            }
        }

        #[cfg(not(CONFIG_DRM_CLIENT))]
        let _ = mode_config_info;

        Ok(Self {
            drm: (&*drm).into(),
            _reg_data: reg_data,
        })
    }

    /// Returns a reference to the `Device` instance for this registration.
    pub fn device(&self) -> &drm::Device<T> {
        &self.drm
    }

    /// Obtain a registered-device view that excludes concurrent parent unbind.
    ///
    /// Returns `None` if the device has been unplugged. Unlike [`Self::device`], the guarded
    /// view permits operations that require completed registration and a bound parent.
    pub fn registration_guard(&self) -> Option<drm::RegistrationGuard<'_, T>> {
        // SAFETY: Registration construction called drm_dev_register successfully.
        let dev = unsafe { self.drm.assume_ctx::<drm::Ioctl>() };
        dev.registration_guard()
    }
}

impl<T: Driver> Registration<'static, T> {
    /// Registers a new [`UnregisteredDevice`](drm::UnregisteredDevice) with owned registration
    /// data.
    ///
    /// Static data does not by itself keep the parent device bound.
    ///
    /// # Safety
    ///
    /// The caller must arrange teardown before release of the parent's bound resources, as
    /// required by [`Registration::new`]. Forgetting it is not permitted while the parent can
    /// subsequently unbind.
    pub unsafe fn new_static<E>(
        dev: &device::Device<device::Bound>,
        drm: drm::UnregisteredDevice<T>,
        reg_data: impl PinInit<T::RegistrationData<'static>, E>,
        flags: usize,
    ) -> Result<Self>
    where
        Error: From<E>,
    {
        // SAFETY: The data cannot contain expiring borrows, and the caller guarantees that
        // registration teardown precedes parent unbind.
        unsafe { Self::new(dev, drm, reg_data, flags) }
    }

    /// Register with owned data for the duration of a callback.
    ///
    /// The callback borrows the registration, so it cannot move or forget the teardown owner.
    /// The framework unplugs the device before returning, while the parent borrow is still bound.
    /// Retained device references remain valid allocations but no longer admit registration guards.
    /// The callback must release its guards before returning and must not block waiting for an
    /// unplug that can only happen after it returns.
    pub fn with_static<E, R>(
        dev: &device::Device<device::Bound>,
        drm: drm::UnregisteredDevice<T>,
        reg_data: impl PinInit<T::RegistrationData<'static>, E>,
        flags: usize,
        callback: impl FnOnce(&Self) -> Result<R>,
    ) -> Result<R>
    where
        Error: From<E>,
    {
        // SAFETY: The local owner cannot escape into the callback. It is dropped on both success
        // and error before the parent borrow ends, including the unplug synchronization barrier.
        let registration = unsafe { Self::new_static(dev, drm, reg_data, flags)? };
        callback(&registration)
    }
}

impl<T: Driver> Drop for Registration<'_, T> {
    fn drop(&mut self) {
        // Use `drm_dev_unplug` rather than `drm_dev_unregister` to ensure that existing
        // `drm_dev_enter()` critical sections complete before unregistration proceeds. This
        // is required for the safety of `RegistrationGuard`, which relies on the SRCU barrier in
        // `drm_dev_unplug()` to guarantee that the parent device is still bound within the
        // critical section.
        //
        // SAFETY: Safe by the invariant of `ARef<drm::Device<T>>`. The existence of this
        // `Registration` also guarantees that this `drm::Device` is actually registered.
        unsafe { bindings::drm_dev_unplug(self.drm.as_raw()) };

        if drm::Device::<T>::has_kms() {
            // SAFETY: KMS was initialized before registration and the device has just been
            // unplugged, matching the teardown order used by removable DRM drivers.
            unsafe { bindings::drm_atomic_helper_shutdown(self.drm.as_raw()) }
        }

        // After drm_dev_unplug(), the SRCU barrier guarantees that all RegistrationGuard critical
        // sections have completed, so no one holds a reference to reg_data anymore.
        // reg_data is dropped here automatically.
    }
}
