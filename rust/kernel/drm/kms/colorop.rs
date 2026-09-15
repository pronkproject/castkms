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

impl<S: FromRawPlaneState> PlaneStateMutator<'_, S> {
    /// Select an advertised non-bypass pipeline by enumeration order, or bypass all operations.
    pub fn select_color_pipeline(&mut self, index: Option<usize>) -> Result {
        // SAFETY: The guard exclusively owns the unpublished plane state.
        let raw = unsafe { self.as_raw_mut() };
        // SAFETY: The guard owns an unpublished plane state. Its plane retains the
        // immutable property enumeration and all static color-operation objects.
        unsafe {
            let mut pipeline = ptr::null_mut();
            if let Some(mut index) = index {
                let property = (*raw.plane).color_pipeline_property;
                if property.is_null() {
                    return Err(EOPNOTSUPP);
                }
                let head = &raw mut (*property).enum_list;
                let mut entry = (*head).next;
                while entry != head {
                    let value =
                        (*crate::container_of!(entry, bindings::drm_property_enum, head)).value;
                    if value != 0 {
                        if index == 0 {
                            let object = bindings::drm_mode_object_find(
                                (*raw.plane).dev,
                                ptr::null_mut(),
                                u32::try_from(value).map_err(|_| EINVAL)?,
                                bindings::DRM_MODE_OBJECT_COLOROP,
                            );
                            if object.is_null() {
                                return Err(EINVAL);
                            }
                            pipeline = crate::container_of!(object, bindings::drm_colorop, base);
                            if (*pipeline).plane != raw.plane {
                                return Err(EINVAL);
                            }
                            break;
                        }
                        index -= 1;
                    }
                    entry = (*entry).next;
                }
                if pipeline.is_null() {
                    return Err(EINVAL);
                }
            }
            let changed = bindings::drm_atomic_set_colorop_for_plane(raw, pipeline);
            raw.set_color_mgmt_changed(raw.color_mgmt_changed() || changed);
        }
        Ok(())
    }

    /// Replace one selected operation in an unpublished candidate.
    ///
    /// Propagate errors to the transaction runner. Operation types and advertised curve
    /// choices are checked; matrix data is copied into a separately owned native blob.
    pub fn set_color_operation(&mut self, index: usize, value: Operation) -> Result {
        // SAFETY: The guard exclusively owns the unpublished plane state.
        let raw = unsafe { self.as_raw_mut() };
        if raw.state.is_null() {
            return Err(EINVAL);
        }
        let mut operation = raw.color_pipeline;
        // SAFETY: The plane guard retains its selected immutable operation chain and
        // modeset lock. Native operation state has no other Rust mutable accessor.
        unsafe {
            for _ in 0..index {
                if operation.is_null() {
                    return Err(EINVAL);
                }
                operation = (*operation).next;
            }
            if operation.is_null() || (*operation).plane != raw.plane {
                return Err(EINVAL);
            }
            let state = from_err_ptr(bindings::drm_atomic_get_colorop_state(raw.state, operation))?;
            match value {
                Operation::Bypass => {
                    if (*operation).bypass_property.is_null() {
                        return Err(EOPNOTSUPP);
                    }
                    (*state).bypass = true;
                }
                Operation::SrgbEotf | Operation::SrgbInverseEotf => {
                    if (*operation).type_ != bindings::drm_colorop_type_DRM_COLOROP_1D_CURVE {
                        return Err(EINVAL);
                    }
                    let curve = if matches!(value, Operation::SrgbEotf) {
                        bindings::drm_colorop_curve_1d_type_DRM_COLOROP_1D_CURVE_SRGB_EOTF
                    } else {
                        bindings::drm_colorop_curve_1d_type_DRM_COLOROP_1D_CURVE_SRGB_INV_EOTF
                    };
                    let property = (*operation).curve_1d_type_property;
                    if property.is_null() {
                        return Err(EINVAL);
                    }
                    let head = &raw mut (*property).enum_list;
                    let mut entry = (*head).next;
                    let mut supported = false;
                    while entry != head {
                        supported |=
                            (*crate::container_of!(entry, bindings::drm_property_enum, head)).value
                                == u64::from(curve);
                        entry = (*entry).next;
                    }
                    if !supported {
                        return Err(EOPNOTSUPP);
                    }
                    (*state).curve_1d_type = curve;
                    (*state).bypass = false;
                }
                Operation::Matrix(coefficients) => {
                    if (*operation).type_ != bindings::drm_colorop_type_DRM_COLOROP_CTM_3X4 {
                        return Err(EINVAL);
                    }
                    let blob = from_err_ptr(bindings::drm_property_create_blob(
                        (*operation).dev,
                        size_of::<[u64; 12]>(),
                        coefficients.as_ptr().cast(),
                    ))?;
                    bindings::drm_property_replace_blob(&raw mut (*state).data, blob);
                    bindings::drm_property_blob_put(blob);
                    (*state).bypass = false;
                }
            }
            raw.set_color_mgmt_changed(true);
        }
        Ok(())
    }

    /// Copy the selected pipeline during atomic validation, without retaining blob pointers.
    ///
    /// The plane-state guard excludes other plane access. Color operations share that
    /// plane's modeset lock. No public color-operation mutator can alias the snapshot.
    pub fn color_pipeline_snapshot(&mut self, maximum: usize) -> Result<KVec<Operation>> {
        let state = self.as_raw();
        if state.state.is_null() {
            return Err(EINVAL);
        }
        let mut operation = state.color_pipeline;
        let mut result = KVec::new();
        while !operation.is_null() {
            if result.len() >= maximum {
                return Err(E2BIG);
            }
            // SAFETY: The selected pipeline is retained by the plane/device. Its
            // immutable links share this plane, whose lock the guard already holds.
            let value = unsafe {
                if (*operation).plane != state.plane {
                    return Err(EINVAL);
                }
                let current = from_err_ptr(bindings::drm_atomic_get_colorop_state(
                    state.state,
                    operation,
                ))?;
                if (*current).bypass {
                    Operation::Bypass
                } else {
                    match (*operation).type_ {
                        bindings::drm_colorop_type_DRM_COLOROP_1D_CURVE => match (*current).curve_1d_type {
                            bindings::drm_colorop_curve_1d_type_DRM_COLOROP_1D_CURVE_SRGB_EOTF => Operation::SrgbEotf,
                            bindings::drm_colorop_curve_1d_type_DRM_COLOROP_1D_CURVE_SRGB_INV_EOTF => Operation::SrgbInverseEotf,
                            _ => return Err(EOPNOTSUPP),
                        },
                        bindings::drm_colorop_type_DRM_COLOROP_CTM_3X4 => {
                            let blob = (*current).data;
                            if blob.is_null() {
                                Operation::Bypass
                            } else {
                                if (*blob).length != size_of::<[u64; 12]>() || (*blob).data.is_null() {
                                    return Err(EINVAL);
                                }
                                Operation::Matrix((*blob).data.cast::<[u64; 12]>().read_unaligned())
                            }
                        },
                        _ => return Err(EOPNOTSUPP),
                    }
                }
            };
            result.push(value, GFP_KERNEL)?;
            // SAFETY: The link is immutable throughout the retained device lifetime.
            operation = unsafe { (*operation).next };
        }
        Ok(result)
    }
}
