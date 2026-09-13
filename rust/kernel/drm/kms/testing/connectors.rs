// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Temporary connector entries in a private device's native object lookup table.

use super::{
    Connector,
    KmsDriver,
    TestDevice, //
};
use crate::{
    drm::kms::ModeObject,
    error::from_err_ptr,
    prelude::*,
    types::ScopeGuard, //
};

/// One published connector identity, without sysfs interfaces or a registered DRM minor.
///
/// Dropping the entry hides the object again while preserving its reserved ID. The borrow
/// prevents private-device shutdown from preceding removal. Drop takes the native object-ID
/// mutex and must not run while an object-access guard holds that lock.
#[must_use = "dropping the entry removes the connector from native object lookup"]
pub struct ConnectorEntry<'a, T: KmsDriver> {
    connector: &'a Connector<T::Connector>,
    fixture: &'a TestDevice<T>,
}

impl<T: KmsDriver> TestDevice<T> {
    /// Publish the only connector's reserved identity for private object-access tests.
    ///
    /// Mode setup reserves its ID without making the object discoverable. This operation
    /// installs the lookup entry under the native lock, without publishing device nodes,
    /// connector sysfs interfaces or hotplug events. Duplicate publication returns `EEXIST`.
    /// Call without modeset, master or object-ID locks held.
    pub fn publish_connector_identity(&self) -> Result<ConnectorEntry<'_, T>> {
        let connector = self.connector()?;
        let object = connector.raw_mode_obj();
        let dev = self.device().as_raw();
        // SAFETY: The exclusively owned initialized topology keeps both native fields alive.
        let (mutex, objects) = unsafe {
            (
                &raw mut (*dev).mode_config.idr_mutex,
                &raw mut (*dev).mode_config.object_idr,
            )
        };
        // SAFETY: The private device retains this initialized mutex through the scope.
        unsafe { bindings::mutex_lock(mutex) };
        let _unlock = ScopeGuard::new(move || {
            // SAFETY: Balance the acquisition on the same task and retained device.
            unsafe { bindings::mutex_unlock(mutex) };
        });
        // SAFETY: The borrowed connector remains initialized; the lock stabilizes its ID.
        let id = unsafe { (*object).id };
        if id == 0 {
            return Err(EINVAL);
        }
        // SAFETY: Native lookup and replacement are serialized by the object-ID mutex.
        if !unsafe { bindings::idr_find(objects, id as _) }.is_null() {
            return Err(EEXIST);
        }
        // SAFETY: Replace only the connector's existing reserved slot. A missing reservation
        // returns an error, and success leaves the borrowed object alive until entry removal.
        from_err_ptr(unsafe { bindings::idr_replace(objects, object.cast(), id as _) })?;
        Ok(ConnectorEntry {
            connector,
            fixture: self,
        })
    }
}

impl<T: KmsDriver> Drop for ConnectorEntry<'_, T> {
    fn drop(&mut self) {
        let object = self.connector.raw_mode_obj();
        let dev = self.fixture.device().as_raw();
        // SAFETY: The entry borrows the private fixture and its initialized connector. The
        // native lock stabilizes lookup and ID; clear only our matching object, retaining the
        // reserved slot for connector cleanup or a later publication. No references are freed.
        unsafe {
            bindings::mutex_lock(&raw mut (*dev).mode_config.idr_mutex);
            let objects = &raw mut (*dev).mode_config.object_idr;
            let id = (*object).id as _;
            if bindings::idr_find(objects, id) == object.cast() {
                bindings::idr_replace(objects, core::ptr::null_mut(), id);
            }
            bindings::mutex_unlock(&raw mut (*dev).mode_config.idr_mutex);
        }
    }
}
