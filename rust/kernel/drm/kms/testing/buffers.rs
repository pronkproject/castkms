// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Private dumb-buffer exports through the native DRM client and handle cache.

use super::{
    KmsDriver,
    TestDevice, //
};
use crate::{
    dma_buf::DmaBuf,
    error::{
        from_err_ptr,
        to_result, //
    },
    prelude::*,
    sync::aref::ARef,
    types::Opaque, //
};
use core::ptr::NonNull;

struct Client(KBox<Opaque<bindings::drm_client_dev>>);

impl Client {
    fn new<T: KmsDriver>(device: &TestDevice<T>) -> Result<Self> {
        let raw = KBox::new(Opaque::new(Default::default()), GFP_KERNEL)?;
        // SAFETY: The fixture owns completed mode setup; the zeroed client has stable storage.
        // Successful initialization retains its own device/file references. No client callbacks
        // are registered, and native initialization unwinds its own failures.
        to_result(unsafe {
            bindings::drm_client_init(
                device.device().as_raw(),
                raw.get(),
                c"rust-test-export".as_char_ptr(),
                core::ptr::null(),
            )
        })?;
        Ok(Self(raw))
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // SAFETY: The initialized private client has no registered callbacks. Release its
        // native file, handles and modeset resources before freeing stable client storage.
        unsafe { bindings::drm_client_release(self.0.get()) };
    }
}

impl<T: KmsDriver> TestDevice<T> {
    /// Export a newly allocated private dumb buffer without installing any descriptors.
    ///
    /// The native internal client supplies the driver's initialized DRM file and handle cache;
    /// export uses the same PRIME lifetime machinery as handle-based userspace export. The
    /// client and its handles close before return, leaving the independently retained DMA-BUF.
    /// Pixels are uninitialized, and the fixture grants no capture authority over other buffers.
    pub fn export_dumb(&self, width: u32, height: u32, bpp: u32) -> Result<ARef<DmaBuf>> {
        let client = Client::new(self)?;
        let device = self.device().as_raw();
        // SAFETY: The initialized client owns a file for this exact retained device.
        let file = unsafe { (*client.0.get()).file };
        // SAFETY: Driver callbacks are immutable and retained by the fixture's device.
        let create = unsafe { (*(*device).driver).dumb_create }.ok_or(EOPNOTSUPP)?;
        let mut args = bindings::drm_mode_create_dumb {
            width,
            height,
            bpp,
            ..Default::default()
        };
        // SAFETY: The callback receives its matching device/file and exclusive writable args.
        to_result(unsafe { create(file, device, &mut args) })?;
        // SAFETY: Export the new handle through native PRIME's file/cache protocol. It returns
        // an owned DMA-BUF reference; no descriptor is installed in a task's file table.
        let raw = from_err_ptr(unsafe {
            bindings::drm_gem_prime_handle_to_dmabuf(device, file, args.handle, bindings::O_RDWR)
        })?;
        let raw = NonNull::new(raw).ok_or(ENOMEM)?;
        // SAFETY: Native export returned one reference to an initialized DMA-BUF.
        Ok(unsafe { DmaBuf::from_owned_raw(raw) })
    }
}
