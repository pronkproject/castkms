// SPDX-License-Identifier: GPL-2.0 OR MIT

//! File-issued grant dispatch, separate from kernel authority and fd publication.

use super::FilePair;
use crate::{
    drm::{
        device::{
            Device,
            Ioctl,
            Registered, //
        },
        file::File as DrmFile,
        kms::KmsDriver, //
    },
    error::to_result,
    fs::File,
    prelude::*,
    sync::aref::ARef, //
};
use core::{
    num::NonZeroU32,
    ptr::NonNull, //
};

/// Nonzero display object IDs, not resolved objects or capture permission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    crtc: NonZeroU32,
    connector: NonZeroU32,
}

/// Checked capture-grant issuance origin selected by the generic dispatcher.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    /// The issuing file must be the exact current DRM master.
    Master,
    /// Initial-user-namespace administration, subject to explicit provider support.
    Administrative,
}

impl Origin {
    fn from_flags(flags: u32) -> Result<Self> {
        match flags {
            0 => Ok(Self::Master),
            crate::uapi::DRM_CAPTURE_GRANT_CREATE_ADMIN => Ok(Self::Administrative),
            _ => Err(EINVAL),
        }
    }

    fn flags(self) -> u32 {
        match self {
            Self::Master => 0,
            Self::Administrative => crate::uapi::DRM_CAPTURE_GRANT_CREATE_ADMIN,
        }
    }
}

impl Target {
    /// Describe the requested CRTC and connector. The provider must resolve and authorize them.
    pub fn new(crtc_id: u32, connector_id: u32) -> Result<Self> {
        Ok(Self {
            crtc: NonZeroU32::new(crtc_id).ok_or(EINVAL)?,
            connector: NonZeroU32::new(connector_id).ok_or(EINVAL)?,
        })
    }

    /// CRTC ID on the issuing file's device.
    pub fn crtc_id(self) -> u32 {
        self.crtc.get()
    }

    /// Connector ID on the issuing file's device.
    pub fn connector_id(self) -> u32 {
        self.connector.get()
    }

    fn raw(self) -> bindings::drm_capture_target {
        bindings::drm_capture_target {
            crtc_id: self.crtc_id(),
            connector_id: self.connector_id(),
        }
    }
}

impl<T: KmsDriver> Device<T, Registered> {
    /// Ask this device's provider to issue creator-bound final-image capture from an open file.
    ///
    /// The provider validates current master role, target access and creator lifetime. A file
    /// on another device is rejected, and absent provider support returns EOPNOTSUPP. The
    /// returned pair installs no descriptors and authorizes no later source read by itself.
    /// Call outside DRM, admission and provider cleanup locks.
    pub fn create_capture_grant(
        &self,
        file: &DrmFile<T::File>,
        target: Target,
    ) -> Result<FilePair> {
        self.create_capture_grant_from(file, target, Origin::Master)
    }

    /// Ask the provider to issue capture through one explicitly selected authority origin.
    pub fn create_capture_grant_from(
        &self,
        file: &DrmFile<T::File>,
        target: Target,
        origin: Origin,
    ) -> Result<FilePair> {
        let mut files = bindings::drm_capture_files::default();
        // SAFETY: Registration and the typed open file retain valid native objects; target
        // and output storage remain live. Native success returns two matching owned files.
        to_result(unsafe {
            bindings::drm_capture_create_file_grant(
                self.as_raw(),
                file.as_raw(),
                &target.raw(),
                origin.flags(),
                &mut files,
            )
        })?;
        // SAFETY: Success transfers initialized client/control file references. Their native
        // operations do not use file-position state, preserving File's thread-safe invariant.
        let capture =
            unsafe { ARef::<File>::from_raw(NonNull::new_unchecked(files.capture.cast())) };
        // SAFETY: The same success contract transfers the separate control reference.
        let control =
            unsafe { ARef::<File>::from_raw(NonNull::new_unchecked(files.control.cast())) };
        FilePair::from_files(&capture, &control)
    }
}

pub(crate) unsafe extern "C" fn create_callback<T: KmsDriver>(
    raw_dev: *mut bindings::drm_device,
    raw_file: *mut bindings::drm_file,
    raw_target: *const bindings::drm_capture_target,
    flags: u32,
    output: *mut bindings::drm_capture_files,
) -> core::ffi::c_int {
    // SAFETY: KMS construction binds this callback to T. Native dispatch retains an open
    // file on that device and verifies registration before invoking the callback.
    let dev = unsafe { Device::<T, Ioctl>::from_raw(raw_dev) };
    let Some(registered) = dev.registration_guard() else {
        return ENODEV.to_errno();
    };
    // SAFETY: Dispatch supplies an open file on this exact driver and callback-local input.
    let file = unsafe { DrmFile::<T::File>::from_raw(raw_file) };
    // SAFETY: Native dispatch keeps this initialized target live for the call.
    let raw_target = unsafe { &*raw_target };
    let target = match Target::new(raw_target.crtc_id, raw_target.connector_id) {
        Ok(target) => target,
        Err(error) => return error.to_errno(),
    };
    let origin = match Origin::from_flags(flags) {
        Ok(origin) => origin,
        Err(error) => return error.to_errno(),
    };
    let pair = match registered
        .registration_data_with(|data| {
            T::create_capture_grant(&registered, data, file, target, origin)
        })
    {
        Ok(pair) => pair,
        Err(error) => return error.to_errno(),
    };
    let (capture, control) = pair.into_files();
    // SAFETY: The caller supplies writable, empty output storage. Transfer each owned file
    // exactly once; the native dispatcher validates and owns cleanup after this return.
    unsafe {
        output.write(bindings::drm_capture_files {
            capture: ARef::into_raw(capture).cast().as_ptr(),
            control: ARef::into_raw(control).cast().as_ptr(),
        });
    }
    0
}

#[cfg(CONFIG_KUNIT)]
#[kunit_tests(rust_drm_capture_target)]
mod tests {
    use super::*;

    #[test]
    fn zero_ids_do_not_form_a_target() -> Result {
        if Target::new(0, 1) != Err(EINVAL) || Target::new(1, 0) != Err(EINVAL) {
            return Err(EINVAL);
        }
        Ok(())
    }

    #[test]
    fn target_retains_ids_without_resolving_objects() -> Result {
        let target = Target::new(7, u32::MAX)?;
        let raw = target.raw();
        if raw.crtc_id != 7 || raw.connector_id != u32::MAX {
            return Err(EINVAL);
        }
        Ok(())
    }
}
