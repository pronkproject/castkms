// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Owned internal DRM clients for test devices, without registered client callbacks.

use super::{
    KmsDriver,
    TestDevice, //
};
use crate::{
    drm::{
        device::{
            Device,
            Registered, //
        },
        file::File, //
    },
    error::to_result,
    prelude::*,
    types::Opaque, //
};
use core::marker::PhantomData;

pub(super) struct Client<T: KmsDriver> {
    raw: KBox<Opaque<bindings::drm_client_dev>>,
    _driver: PhantomData<T>,
}

impl<T: KmsDriver> Client<T> {
    pub(super) fn new(device: &TestDevice<T>) -> Result<Self> {
        Self::new_on(device.device())
    }

    pub(super) fn new_registered(device: &Device<T, Registered>) -> Result<Self> {
        Self::new_on(device)
    }

    // Both entry points retain completed KMS setup through native client initialization.
    fn new_on(device: &Device<T>) -> Result<Self> {
        let raw = KBox::new(Opaque::new(Default::default()), GFP_KERNEL)?;
        // SAFETY: The fixture owns completed mode setup; the zeroed client has stable storage.
        // Successful initialization retains its own device/file references. No client callbacks
        // are registered, and native initialization unwinds its own failures.
        to_result(unsafe {
            bindings::drm_client_init(
                device.as_raw(),
                raw.get(),
                c"rust-kms-test-client".as_char_ptr(),
                core::ptr::null(),
            )
        })?;
        Ok(Self {
            raw,
            _driver: PhantomData,
        })
    }

    pub(super) fn file(&self) -> &File<T::File> {
        // SAFETY: Native initialization opened this file on the matching driver's device.
        // The client owns it until release; the borrow cannot outlive the client.
        unsafe { File::from_raw((*self.raw.get()).file) }
    }
}

impl<T: KmsDriver> Drop for Client<T> {
    fn drop(&mut self) {
        // SAFETY: The initialized private client has no registered callbacks. Release its
        // native file, handles and modeset resources before freeing stable client storage.
        unsafe { bindings::drm_client_release(self.raw.get()) };
    }
}
