// SPDX-License-Identifier: GPL-2.0 OR MIT

use crate::{
    drm::{
        device::{
            Device,
            Registered, //
        },
        file::File,
        kms::{
            connector::{
                AsRawConnector,
                Connector, //
            },
            KmsDriver, //
        },
    },
    prelude::*,
    sync::aref::ARef, //
};
use core::ptr::NonNull;

impl<T: KmsDriver> Device<T, Registered> {
    /// Retain a connector visible to an open file on this device.
    ///
    /// The result owns both the native connector and its device, not permission to use
    /// either. Call outside the object-ID lock. Unknown, inaccessible or wrong-type IDs
    /// return ENOENT; later operations must revalidate policy under their own exclusion.
    pub fn lookup_connector(
        &self,
        file: &File<T::File>,
        id: u32,
    ) -> Result<ARef<Connector<T::Connector>>> {
        if self.as_raw() != file.device_raw() {
            return Err(EINVAL);
        }
        // SAFETY: The registered device and its typed file remain live. Native lookup
        // checks type and lease, and acquires one connector reference before unlocking.
        let object = unsafe {
            bindings::drm_mode_object_find(
                self.as_raw(),
                file.as_raw(),
                id,
                bindings::DRM_MODE_OBJECT_CONNECTOR,
            )
        };
        if object.is_null() {
            return Err(ENOENT);
        }
        // SAFETY: The type filter establishes the container, and Rust construction
        // enforces T::Connector. The native lookup reference retains this connector.
        let connector = unsafe {
            Connector::<T::Connector>::from_raw(crate::container_of!(
                object,
                bindings::drm_connector,
                base
            ))
        };
        // SAFETY: Each Rust connector ARef owns a device reference in addition to its
        // native object reference. Add the former and transfer the lookup's latter;
        // no fallible operation intervenes before the ARef owns both references.
        unsafe {
            bindings::drm_dev_get(self.as_raw());
            Ok(ARef::from_raw(NonNull::from(connector)))
        }
    }
}
