// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Allocation and native callback ownership for typed framebuffer metadata.

use super::*;
use crate::error::to_result;

// The native framebuffer is first so its destructor recovers the enclosing allocation.
// Metadata is fully initialized before DRM publishes the native framebuffer.
#[repr(C)]
struct Storage<T: KmsDriver> {
    base: Opaque<bindings::drm_framebuffer>,
    data: T::FramebufferData,
}

impl<T: KmsDriver> Storage<T> {
    fn new(data: T::FramebufferData) -> Result<KBox<Self>> {
        Ok(KBox::new(
            Self {
                base: Opaque::zeroed(),
                data,
            },
            GFP_KERNEL,
        )?)
    }

    unsafe extern "C" fn destroy(raw: *mut bindings::drm_framebuffer) {
        // SAFETY: Only a successfully initialized Storage installs this callback. DRM calls
        // it with exclusive ownership after the final framebuffer reference is released.
        let storage = unsafe { KBox::from_raw(raw.cast::<Self>()) };
        let raw = storage.base.get();
        // SAFETY: Successful GEM initialization owns one object reference per format plane.
        unsafe {
            for index in 0..(*(*raw).format).num_planes as usize {
                bindings::drm_gem_object_put((*raw).obj[index]);
            }
            bindings::drm_framebuffer_cleanup(raw);
        }
        drop(storage);
    }
}

pub(crate) const fn vtable<T: KmsDriver>() -> bindings::drm_framebuffer_funcs {
    bindings::drm_framebuffer_funcs {
        destroy: Some(Storage::<T>::destroy),
        create_handle: Some(bindings::drm_gem_fb_create_handle),
        dirty: None,
    }
}

/// # Safety
///
/// The device must have completed Rust mode-configuration setup.
unsafe fn device_vtable<T: KmsDriver>(dev: &Device<T>) -> &bindings::drm_framebuffer_funcs {
    // SAFETY: Rust KMS setup installs a pointer to the kms_vtable field of ModeConfigOps.
    // The selected allocation remains alive and unchanged throughout the device lifetime.
    // Both construction and lookup must follow that installed pointer to the same allocation.
    unsafe {
        let ops = crate::container_of!(
            (*dev.as_raw()).mode_config.funcs,
            super::super::ModeConfigOps,
            kms_vtable
        );
        &(*ops).framebuffer_vtable
    }
}

pub(super) fn data<T: KmsDriver>(fb: &Framebuffer<T>) -> Option<&T::FramebufferData> {
    let raw = fb.as_raw();
    // SAFETY: The framebuffer is live. Matching our callbacks identifies the enclosing
    // allocation; the typed device identifies T. Other native constructors return None.
    unsafe {
        if !ptr::eq((*raw).funcs, device_vtable(fb.drm_dev())) {
            return None;
        }
        Some(&(*raw.cast::<Storage<T>>()).data)
    }
}

/// The caller protects initialized mode configuration and the borrowed object array.
pub(super) unsafe fn from_objects<T: KmsDriver>(
    dev: &Device<T>,
    command: &bindings::drm_mode_fb_cmd2,
    objects: &[*mut bindings::drm_gem_object],
    data: T::FramebufferData,
) -> Result<*mut bindings::drm_framebuffer> {
    let storage = Storage::<T>::new(data)?;
    // SAFETY: The caller protects KMS setup and all objects. The allocation is zeroed and
    // privately owned; failure releases acquired object references without publishing it.
    to_result(unsafe {
        bindings::drm_gem_fb_init_from_objects(
            dev.as_raw(),
            storage.base.get(),
            command,
            objects.as_ptr(),
            objects.len() as u32,
            device_vtable(dev),
        )
    })?;
    Ok(KBox::into_raw(storage).cast())
}
