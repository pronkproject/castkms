// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Owned color-operation descriptions and setup for an sRGB/matrix plane pipeline.

use super::{plane::*, KmsDriver};
use crate::{
    bindings,
    error::{from_err_ptr, to_result},
    prelude::*,
};
use core::{mem::size_of, ptr};

/// An immutable operation, with no reference to mutable KMS state or property blobs.
#[derive(Clone, Copy)]
pub enum Operation {
    /// Preserve input values unchanged.
    Bypass,
    /// Decode the sRGB transfer function.
    SrgbEotf,
    /// Encode the sRGB transfer function.
    SrgbInverseEotf,
    /// Three rows of four S31.32 sign-magnitude coefficients, including offsets.
    Matrix([u64; 12]),
}

static FUNCS: bindings::drm_colorop_funcs = bindings::drm_colorop_funcs {
    destroy: Some(bindings::drm_colorop_destroy),
};

impl<T: DriverPlane> UnregisteredPlane<T>
where
    T::Driver: KmsDriver<Plane = T>,
{
    /// Attach curve, matrix, matrix, curve operations, each independently bypassable.
    ///
    /// Curves support sRGB EOTF and its inverse. Successfully initialized operations
    /// are owned by mode configuration, including when a later setup step fails.
    pub fn create_srgb_matrix_pipeline(&self) -> Result {
        let mut first = ptr::null_mut();
        let mut previous = ptr::null_mut();
        for index in 0..4 {
            let mut operation = KBox::new(bindings::drm_colorop::default(), GFP_KERNEL)?;
            let raw: *mut bindings::drm_colorop = &mut *operation;
            // SAFETY: The exclusive setup view retains plane/device lifetime. The
            // operation is zeroed owned storage, and FUNCS is permanently live.
            let result = unsafe {
                let dev = (*self.as_raw()).dev;
                if index == 0 || index == 3 {
                    bindings::drm_plane_colorop_curve_1d_init(dev, raw, self.as_raw(), &FUNCS,
                        (1 << bindings::drm_colorop_curve_1d_type_DRM_COLOROP_1D_CURVE_SRGB_EOTF)
                        | (1 << bindings::drm_colorop_curve_1d_type_DRM_COLOROP_1D_CURVE_SRGB_INV_EOTF),
                        bindings::DRM_COLOROP_FLAG_ALLOW_BYPASS)
                } else {
                    bindings::drm_plane_colorop_ctm_3x4_init(
                        dev,
                        raw,
                        self.as_raw(),
                        &FUNCS,
                        bindings::DRM_COLOROP_FLAG_ALLOW_BYPASS,
                    )
                }
            };
            if !operation.dev.is_null() {
                // Native initialization linked the kmalloc allocation into mode
                // configuration. drm_colorop_destroy will clean it and kfree it.
                let _ = KBox::into_raw(operation);
            }
            to_result(result)?;
            if index == 0 {
                first = raw;
            } else {
                // SAFETY: Both initialized operations belong to this unregistered plane.
                unsafe { bindings::drm_colorop_set_next_property(previous, raw) };
            }
            previous = raw;
        }
        let pipeline = bindings::drm_prop_enum_list {
            // SAFETY: Successful initialization retains the first operation until cleanup.
            type_: unsafe { (*first).base.id } as _,
            name: c"sRGB / matrix / matrix / sRGB".as_char_ptr(),
        };
        // SAFETY: Native property creation copies the enum descriptor before returning.
        to_result(unsafe {
            bindings::drm_plane_create_color_pipeline_property(self.as_raw(), &pipeline, 1)
        })
    }
}
