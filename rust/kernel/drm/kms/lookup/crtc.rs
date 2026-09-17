// SPDX-License-Identifier: GPL-2.0 OR MIT

use crate::{
    drm::{
        device::{
            Device,
            Registered, //
        },
        file::File,
        kms::{
            crtc::{
                AsRawCrtc,
                Crtc,
                CrtcRef, //
            },
            KmsDriver, //
        },
    },
    prelude::*, //
};

impl<T: KmsDriver> Device<T, Registered> {
    fn lookup_crtc_with_file(
        &self,
        file: Option<&File<T::File>>,
        id: u32,
    ) -> Result<CrtcRef<T::Crtc>> {
        if file.is_some_and(|file| self.as_raw() != file.device_raw()) {
            return Err(EINVAL);
        }
        // SAFETY: Registration retains completed KMS setup. A supplied file belongs to this
        // device; NULL deliberately requests an unfiltered device lookup. Native lookup
        // validates the object type under its object-ID lock. CRTCs have static device lifetime.
        let object = unsafe {
            bindings::drm_mode_object_find(
                self.as_raw(),
                file.map_or(core::ptr::null_mut(), File::as_raw),
                id,
                bindings::DRM_MODE_OBJECT_CRTC,
            )
        };
        if object.is_null() {
            return Err(ENOENT);
        }
        // SAFETY: The native type filter establishes the container; Rust construction restricts
        // every CRTC on this device to T::Crtc. The registered device retains the static object.
        let crtc = unsafe {
            Crtc::<T::Crtc>::from_raw(crate::container_of!(object, bindings::drm_crtc, base))
        };
        Ok(crtc.to_owned_ref())
    }

    /// Retain a CRTC visible to an open file on this device.
    ///
    /// Lookup checks object type and the file's lease, not current master role or future
    /// permission. Call outside the object-ID lock; later operations must recheck access
    /// under their own exclusion. Unknown, inaccessible or wrong-type IDs return ENOENT.
    pub fn lookup_crtc(&self, file: &File<T::File>, id: u32) -> Result<CrtcRef<T::Crtc>> {
        self.lookup_crtc_with_file(Some(file), id)
    }

    /// Retain a CRTC by device-global ID without applying one file's lease filter.
    ///
    /// This is identity lookup, not authorization. Callers must independently establish a
    /// policy origin and recheck current access to the returned object under native exclusion.
    pub fn lookup_crtc_unfiltered(&self, id: u32) -> Result<CrtcRef<T::Crtc>> {
        self.lookup_crtc_with_file(None, id)
    }
}
