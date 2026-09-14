// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Owned immutable DRM property data, prepared independently of publication.

use super::KmsDriver;
use crate::{
    bindings,
    drm::device::{
        Device,
        Registered, //
    },
    error::from_err_ptr,
    prelude::*,
    sync::aref::ARef, //
};
use core::ptr::NonNull;

/// One reference to immutable property data, with its mode configuration retained.
///
/// A blob is public DRM metadata, not storage for confidential pixels. Creation copies
/// all bytes before returning and does not attach the blob to a display object. Allocate
/// outside modeset and driver control locks. Drop outside mode-object ID locks, since final
/// release removes the blob from its device's object registry.
///
/// Device-owned state must release owned blobs during explicit shutdown to avoid retaining
/// its own device indefinitely. Unplug does not invalidate independently retained bytes.
pub struct Blob<T: KmsDriver> {
    pub(super) raw: NonNull<bindings::drm_property_blob>,
    pub(super) device: ARef<Device<T>>,
}

// SAFETY: The native blob is immutable and reference counted. The retained device keeps
// mode configuration alive, and native reference release serializes registry removal.
unsafe impl<T: KmsDriver> Send for Blob<T> {}
// SAFETY: Shared access exposes only immutable data; cloning obtains another native reference.
unsafe impl<T: KmsDriver> Sync for Blob<T> {}

impl<T: KmsDriver> Blob<T> {
    /// Copy nonempty metadata into a new blob on a registered device.
    pub fn new(device: &Device<T, Registered>, bytes: &[u8]) -> Result<Self> {
        // SAFETY: Registration establishes initialized mode configuration and excludes teardown.
        unsafe { Self::new_unchecked(device, bytes) }
    }

    /// Create while mode configuration is initialized and teardown is excluded.
    ///
    /// # Safety
    ///
    /// The caller must establish initialized mode configuration, which remains managed by
    /// the retained device until its final release. No modeset or object-ID lock may be held.
    pub(super) unsafe fn new_unchecked(device: &Device<T>, bytes: &[u8]) -> Result<Self> {
        // SAFETY: The caller establishes initialized mode configuration. The input slice is
        // readable and native construction copies it before publishing the blob's object ID.
        let raw = from_err_ptr(unsafe {
            bindings::drm_property_create_blob(device.as_raw(), bytes.len(), bytes.as_ptr().cast())
        })?;
        Ok(Self {
            // SAFETY: Successful native construction returns a nonnull owned reference.
            raw: unsafe { NonNull::new_unchecked(raw) },
            device: device.into(),
        })
    }

    /// The device-local object ID, without a promise that any property refers to this blob.
    pub fn id(&self) -> u32 {
        // SAFETY: The owned reference retains the initialized native object and immutable ID.
        unsafe { (*self.raw.as_ptr()).base.id }
    }

    /// Borrow the exact immutable bytes copied during construction.
    pub fn as_bytes(&self) -> &[u8] {
        // SAFETY: Native construction initializes this nonempty allocation and copies all
        // bytes. The blob reference retains it; no API mutates its data after publication.
        unsafe {
            core::slice::from_raw_parts(
                (*self.raw.as_ptr()).data.cast(),
                (*self.raw.as_ptr()).length,
            )
        }
    }
}

impl<T: KmsDriver> Clone for Blob<T> {
    fn clone(&self) -> Self {
        // SAFETY: The original reference remains live while the new native reference is taken.
        unsafe { bindings::drm_property_blob_get(self.raw.as_ptr()) };
        Self {
            raw: self.raw,
            device: self.device.clone(),
        }
    }
}

impl<T: KmsDriver> Drop for Blob<T> {
    fn drop(&mut self) {
        // SAFETY: Release exactly this owner's reference before releasing its device field.
        unsafe { bindings::drm_property_blob_put(self.raw.as_ptr()) };
    }
}
