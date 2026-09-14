// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Connector blob descriptions attached during object construction.

mod readonly_blob;
pub use readonly_blob::ReadOnlyBlobProperty;

use super::{
    AsRawConnector,
    DriverConnector,
    UnregisteredConnector, //
};
use crate::{
    bindings,
    error::from_err_ptr,
    prelude::*, //
};
use core::ptr::NonNull;

impl<T: DriverConnector> UnregisteredConnector<T> {
    /// Attach a read-only blob whose bytes remain fixed for the connector's lifetime.
    ///
    /// The bytes are copied before publication. Mode configuration owns the property
    /// and the blob's initial reference, releasing both during its cleanup. No Rust
    /// object retains the device, and callers must not encode uninitialized padding.
    /// This is a static description, not an interface for changing capabilities.
    pub fn attach_static_blob_property(&self, name: &CStr, bytes: &[u8]) -> Result {
        attach_blob(self, name, bytes).map(|_| ())
    }
}

// Mode configuration owns both the property and the blob's initial reference after success.
// Attachment happens only during construction; returned pointers do not add ownership.
fn attach_blob<T: DriverConnector>(
    connector: &UnregisteredConnector<T>,
    name: &CStr,
    bytes: &[u8],
) -> Result<(
    NonNull<bindings::drm_property>,
    NonNull<bindings::drm_property_blob>,
)> {
    if name.is_empty() || name.to_bytes().len() >= bindings::DRM_PROP_NAME_LEN as usize {
        return Err(EINVAL);
    }
    let raw = connector.as_raw();
    // SAFETY: Construction owns the initialized connector exclusively until registration.
    // The mode object and its property array remain live throughout the call.
    let (dev, object) = unsafe { ((*raw).dev, &raw mut (*raw).base) };
    // SAFETY: The initialized property array is not concurrently modified before publication.
    if unsafe { (*(*object).properties).count } >= bindings::DRM_OBJECT_MAX_PROPERTY as i32 {
        return Err(ENOSPC);
    }
    // SAFETY: Mode configuration is initialized, and bytes is readable for its entire length.
    let blob = from_err_ptr(unsafe {
        bindings::drm_property_create_blob(dev, bytes.len(), bytes.as_ptr().cast())
    })?;
    // SAFETY: The name is terminated, and mode configuration owns newly created properties.
    let property = unsafe {
        bindings::drm_property_create(
            dev,
            bindings::DRM_MODE_PROP_BLOB | bindings::DRM_MODE_PROP_IMMUTABLE,
            name.as_char_ptr(),
            0,
        )
    };
    if property.is_null() {
        // SAFETY: The unpublished blob still owns its sole reference.
        unsafe { bindings::drm_property_blob_put(blob) };
        return Err(ENOMEM);
    }
    // SAFETY: The property array has capacity, both objects belong to dev, and the blob is
    // fully initialized. Mode-config cleanup releases the initial blob reference after
    // connector destruction, so the property never refers to freed storage while published.
    unsafe { bindings::drm_object_attach_property(object, property, u64::from((*blob).base.id)) };
    // SAFETY: Both native constructors succeeded and attachment transfers no references.
    Ok(unsafe {
        (
            NonNull::new_unchecked(property),
            NonNull::new_unchecked(blob),
        )
    })
}
