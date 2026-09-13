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

impl<T: KmsDriver> MasterFile<'_, T> {
    /// Borrow the initialized file without extending its ownership lifetime.
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
        let dev = self.device().as_raw();
        let file = client.file().as_raw();
        let raw = allocate_master(dev)?;
        // SAFETY: The native constructor returned one reference on this retained device.
        let _identity = unsafe { MasterRef::from_owned_raw(raw, self.device()) };
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
        Ok(MasterFile {
            client,
            _fixture: self,
        })
    }
}
