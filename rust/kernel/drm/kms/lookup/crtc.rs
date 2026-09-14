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
    /// Retain a CRTC visible to an open file on this device.
    ///
    /// Lookup checks object type and the file's lease, not current master role or future
    /// permission. Call outside the object-ID lock; later operations must recheck access
    /// under their own exclusion. Unknown, inaccessible or wrong-type IDs return ENOENT.
    pub fn lookup_crtc(&self, file: &File<T::File>, id: u32) -> Result<CrtcRef<T::Crtc>> {
        if self.as_raw() != file.device_raw() {
            return Err(EINVAL);
        }
        // SAFETY: Registration retains completed KMS setup and file belongs to this device.
        // Native lookup filters the type and lease under its object-ID lock. CRTCs have
        // static device lifetime, so lookup does not return a separate object reference.
        let object = unsafe {
            bindings::drm_mode_object_find(
                self.as_raw(),
                file.as_raw(),
                id,
                bindings::DRM_MODE_OBJECT_CRTC,
            )
        };
        if object.is_null() {
            return Err(ENOENT);
        }
        // SAFETY: The type filter establishes the native container; Rust constructors
        // restrict every CRTC on this device to T::Crtc. self retains the static object.
        let crtc = unsafe {
            Crtc::<T::Crtc>::from_raw(crate::container_of!(object, bindings::drm_crtc, base))
        };
        Ok(crtc.to_owned_ref())
    }
}
