// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Real master-file ownership on private, unregistered test devices.

use super::{
    allocate_master,
    client::Client,
    KmsDriver,
    TestDevice, //
};
use crate::{
    drm::{
        auth::MasterRef,
        file::File, //
    },
    prelude::*,
    types::ScopeGuard, //
};

/// An internal master file whose native close precedes private-device shutdown.
///
/// No userspace descriptor or DRM minor is installed. Creation and final close invoke
/// the driver's real master callbacks. Retaining a master snapshot does not retain this
/// file or postpone its close callback.
/// Drop may sleep and must run without modeset, master or object-ID locks held.
///
/// ```compile_fail
/// use kernel::{
///     drm::kms::{testing::{MasterFile, TestDevice}, KmsDriver},
///     prelude::*,
/// };
/// fn outlive_device<'a, T: KmsDriver>(device: TestDevice<T>) -> Result<MasterFile<'a, T>> {
///     device.master_file()
/// }
/// ```
pub struct MasterFile<'a, T: KmsDriver> {
    client: Client<T>,
    _fixture: &'a TestDevice<T>,
}

impl<'a, T: KmsDriver> MasterFile<'a, T> {
    /// Borrow the initialized file without extending its ownership lifetime.
    pub fn file(&self) -> &File<T::File> {
        self.client.file()
    }

    /// Open a private peer with the same identity but without the master file's role.
    ///
    /// Call outside native master, object-ID and modeset locks. The owner must still be
    /// current. The peer owns its association and may outlive the master file, but not the
    /// fixture. Creation does not invoke a master-set callback or grant display control.
    pub fn associated_file(&self) -> Result<AssociatedFile<'a, T>> {
        let client = Client::new(self._fixture)?;
        let identity = self.file().associated_master().ok_or(EINVAL)?;
        {
            let access = identity.lock_current().ok_or(EACCES)?;
            if !access.is_master_file(self.file()) {
                return Err(EACCES);
            }
            let peer = client.file().as_raw();
            let owner = self.file().as_raw();
            // SAFETY: The new client retains its initialized file but has no association.
            // The owner's native role and association are stabilized by the access guard.
            // Publish an independently retained association under the lookup lock. Native
            // peer close releases it; the zero-initialized master role remains false.
            unsafe {
                bindings::spin_lock(&raw mut (*peer).master_lookup_lock);
                (*peer).master = bindings::drm_master_get((*owner).master);
                bindings::spin_unlock(&raw mut (*peer).master_lookup_lock);
            }
        }
        Ok(AssociatedFile {
            client,
            _fixture: self._fixture,
        })
    }
}

/// A private non-master file retaining an association, not its creating master file.
///
/// No descriptor is installed. Closing the file follows native client cleanup without
/// dropping another file's display control. The fixture must outlive the peer.
pub struct AssociatedFile<'a, T: KmsDriver> {
    client: Client<T>,
    _fixture: &'a TestDevice<T>,
}

impl<T: KmsDriver> AssociatedFile<'_, T> {
    /// Borrow the initialized non-master file without extending its close lifetime.
    pub fn file(&self) -> &File<T::File> {
        self.client.file()
    }
}

impl<T: KmsDriver> TestDevice<T> {
    /// Create the current top-level master file on the private device.
    ///
    /// Returns `EBUSY` if another master is current. Call without modeset, master or
    /// object-ID locks. The file borrows the fixture so its native close and master-drop
    /// callback finish before private-device shutdown. No capture grant is created.
    pub fn master_file(&self) -> Result<MasterFile<'_, T>> {
        let client = Client::new(self)?;
        install_master(&client)?;
        Ok(MasterFile {
            client,
            _fixture: self,
        })
    }
}

// Both fixture types own an initialized internal primary file during installation.
pub(super) fn install_master<T: KmsDriver>(client: &Client<T>) -> Result {
    let dev = client.file().device_raw();
    let file = client.file().as_raw();
    let raw = allocate_master(dev)?;
    // SAFETY: The initialized typed client retains its matching DRM device.
    let device = unsafe { crate::drm::Device::<T>::from_raw(dev) };
    // SAFETY: The native constructor returned one reference on this retained device.
    let _identity = unsafe { MasterRef::from_owned_raw(raw, device) };
    {
        // SAFETY: The fixture and internal client retain the initialized native device.
        let mutex = unsafe { &raw mut (*dev).master_mutex };
        // SAFETY: The retained device keeps its initialized mutex alive throughout this scope.
        unsafe { bindings::mutex_lock(mutex) };
        let _unlock = ScopeGuard::new(move || {
            // SAFETY: This scope owns one acquisition on the same task and live device.
            unsafe { bindings::mutex_unlock(mutex) };
        });
        // SAFETY: The master mutex stabilizes device ownership. The freshly opened client
        // has no master association and has not been exposed through this fixture API.
        if unsafe { !(*dev).master.is_null() || !(*file).master.is_null() } {
            return Err(EBUSY);
        }
        // SAFETY: Install independently retained file/device references under native locks.
        // The file lookup spinlock protects association readers. The immutable driver
        // callback runs after installation, under the native master-set lock contract.
        // Native client close releases both references and performs master-drop cleanup.
        unsafe {
            bindings::spin_lock(&raw mut (*file).master_lookup_lock);
            (*file).master = bindings::drm_master_get(raw.as_ptr());
            bindings::spin_unlock(&raw mut (*file).master_lookup_lock);
            (*file).is_master = true;
            (*file).authenticated = true;
            (*dev).master = bindings::drm_master_get(raw.as_ptr());
            if let Some(callback) = (*(*dev).driver).master_set {
                callback(dev, file, true);
            }
            (*file).was_master = true;
        }
    }
    Ok(())
}
