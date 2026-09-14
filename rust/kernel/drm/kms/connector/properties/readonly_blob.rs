// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Driver replacement of a userspace-read-only blob without allocation under control locks.

use super::*;
use crate::{
    drm::kms::{
        blob::Blob,
        connector::Connector,
        LockedState,
        ModeObject, //
    },
    error::to_result,
    sync::aref::ARef, //
};

/// Unique driver control of a connector blob property that userspace cannot set.
///
/// The handle retains its connector and device. Mode configuration owns the property and
/// its installed blob reference, including after this handle is dropped. Driver-owned state
/// must drop the handle during explicit shutdown to avoid retaining its own device forever.
///
/// # Invariants
///
/// The property is attached to `connector` and changed only through this handle. `current`
/// identifies its installed, mode-configuration-owned blob reference. Replacement exchanges
/// that reference with an independently owned Blob without changing reference counts.
pub struct ReadOnlyBlobProperty<T: DriverConnector> {
    connector: ARef<Connector<T>>,
    property: NonNull<bindings::drm_property>,
    current: NonNull<bindings::drm_property_blob>,
}

// SAFETY: Retained connector ownership keeps mode configuration alive across task transfer.
// Mutation requires exclusive handle access and the same device's modeset locks.
unsafe impl<T: DriverConnector> Send for ReadOnlyBlobProperty<T> {}
// SAFETY: Shared access reads only the immutable property ID. Replacement requires &mut self.
unsafe impl<T: DriverConnector> Sync for ReadOnlyBlobProperty<T> {}

impl<T: DriverConnector> UnregisteredConnector<T> {
    /// Attach an initial read-only blob and return its unique driver replacement handle.
    ///
    /// The initial bytes are copied before attachment. Userspace cannot set the property,
    /// but the driver may replace its blob under modeset control. Dropping the handle leaves
    /// the installed description valid until ordinary mode-configuration cleanup.
    pub fn attach_readonly_blob_property(
        &self,
        name: &CStr,
        bytes: &[u8],
    ) -> Result<ReadOnlyBlobProperty<T>> {
        let (property, current) = attach_blob(self, name, bytes)?;
        // SAFETY: Construction retains this initialized connector. Acquire paired native
        // connector/device references before returning the independently owned handle.
        let connector = unsafe { Connector::<T>::from_raw(self.as_raw()) }.into();
        Ok(ReadOnlyBlobProperty {
            connector,
            property,
            current,
        })
    }
}

impl<T: DriverConnector> ReadOnlyBlobProperty<T> {
    /// The immutable property object ID, not the currently installed blob ID.
    pub fn id(&self) -> u32 {
        // SAFETY: The retained connector/device keep the property and its immutable ID alive.
        unsafe { (*self.property.as_ptr()).base.id }
    }

    /// Publish preallocated metadata, leaving the retired blob in `replacement`.
    ///
    /// Both the lock view and replacement must belong to this handle's device. Errors leave
    /// the property and replacement unchanged. Success performs no allocation or reference
    /// release: the caller drops the retired replacement after leaving outer control locks.
    /// Native modeset readers observe the new ID after those locks are released.
    pub fn replace_blob(
        &mut self,
        state: &LockedState<'_, T::Driver>,
        replacement: &mut Blob<T::Driver>,
    ) -> Result {
        let device = self.connector.drm_dev().as_raw();
        if state.device().as_raw() != device || replacement.device.as_raw() != device {
            return Err(EINVAL);
        }
        // SAFETY: The view holds this device's modeset locks. The uniquely controlled
        // property remains attached to the retained connector and the replacement is live.
        to_result(unsafe {
            bindings::drm_object_property_set_value(
                &raw mut (*self.connector.as_raw()).base,
                self.property.as_ptr(),
                u64::from(replacement.id()),
            )
        })?;
        // The old installed reference becomes the caller's owner; the new one belongs to
        // mode configuration. Their unchanged device references still name the same device.
        core::mem::swap(&mut self.current, &mut replacement.raw);
        Ok(())
    }
}
