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
    fn lookup_connector_with_file(
        &self,
        file: Option<&File<T::File>>,
        id: u32,
    ) -> Result<ARef<Connector<T::Connector>>> {
        if file.is_some_and(|file| self.as_raw() != file.device_raw()) {
            return Err(EINVAL);
        }
        // SAFETY: Registration retains completed KMS setup. A supplied file belongs to this
        // device; NULL deliberately requests an unfiltered device lookup. Native lookup checks
        // the type and acquires one connector reference under its object-ID lock.
        let object = unsafe {
            bindings::drm_mode_object_find(
                self.as_raw(),
                file.map_or(core::ptr::null_mut(), File::as_raw),
                id,
                bindings::DRM_MODE_OBJECT_CONNECTOR,
            )
        };
        if object.is_null() {
            return Err(ENOENT);
        }
        // SAFETY: The native type filter establishes the container, and Rust construction
        // enforces T::Connector.
        let connector = unsafe {
            Connector::<T::Connector>::from_raw(crate::container_of!(
                object,
                bindings::drm_connector,
                base
            ))
        };
        // SAFETY: Each Rust connector ARef owns a device reference in addition to its native
        // object reference. Add the former and transfer the lookup's latter without intervening
        // failure.
        unsafe {
            bindings::drm_dev_get(self.as_raw());
            Ok(ARef::from_raw(NonNull::from(connector)))
        }
    }

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
        self.lookup_connector_with_file(Some(file), id)
    }

    /// Retain a connector by device-global ID without applying one file's lease filter.
    ///
    /// This establishes identity and lifetime only. The caller must authorize current access
    /// to the object separately before exposing any capability derived from it.
    pub fn lookup_connector_unfiltered(&self, id: u32) -> Result<ARef<Connector<T::Connector>>> {
        self.lookup_connector_with_file(None, id)
    }
}
