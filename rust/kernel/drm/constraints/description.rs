// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Referenced allocation metadata owned by common DRM code.

use crate::{
    error::from_err_ptr,
    prelude::*,
    sync::aref::{
        ARef,
        AlwaysRefCounted, //
    },
    types::Opaque, //
};
use core::{
    ptr::NonNull,
    slice, //
};

/// Inclusive integer-pixel allocation or output dimensions.
///
/// Construction does not validate bounds; [`Description::new`] validates the complete input.
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct Size(bindings::drm_constraints_size);

impl Size {
    /// Construct inclusive bounds. Minima must be nonzero and no greater than maxima.
    pub const fn new(min_width: u32, min_height: u32, max_width: u32, max_height: u32) -> Self {
        Self(bindings::drm_constraints_size {
            min_width,
            min_height,
            max_width,
            max_height,
        })
    }

    /// Describe a single permitted size.
    pub const fn exact(width: u32, height: u32) -> Self {
        Self::new(width, height, width, height)
    }

    /// Smallest permitted width and height.
    pub const fn minimum(&self) -> (u32, u32) {
        (self.0.min_width, self.0.min_height)
    }

    /// Largest permitted width and height.
    pub const fn maximum(&self) -> (u32, u32) {
        (self.0.max_width, self.0.max_height)
    }
}

/// One plane's format/modifier pair and framebuffer allocation bounds.
///
/// The plane ID identifies an existing KMS object; the description does not retain that object.
/// The provider validates device/output membership when offering an entry.
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct Format(bindings::drm_constraints_format);

impl Format {
    /// Construct allocation metadata using standard DRM fourcc and modifier values.
    pub const fn new(plane_id: u32, format: u32, modifier: u64, size: Size) -> Self {
        Self(bindings::drm_constraints_format {
            plane_id,
            format,
            modifier,
            size: size.0,
        })
    }

    /// DRM plane object ID.
    pub const fn plane_id(&self) -> u32 {
        self.0.plane_id
    }

    /// DRM fourcc.
    pub const fn format(&self) -> u32 {
        self.0.format
    }

    /// DRM format modifier, including linear.
    pub const fn modifier(&self) -> u64 {
        self.0.modifier
    }

    /// Inclusive framebuffer allocation bounds, not fractional source-rectangle bounds.
    pub const fn size(&self) -> Size {
        Size(self.0.size)
    }
}

/// Independently referenced, immutable allocation description.
///
/// These bounds are necessary, not sufficient, for displaying a scene. Construction and final
/// reference release require a context that may sleep.
///
/// # Invariants
///
/// Every reference retains an initialized native description with a live reference count.
#[repr(transparent)]
pub struct Description(pub(super) Opaque<bindings::drm_constraints_description>);

// SAFETY: Native reference counting is thread-safe and the payload is immutable.
unsafe impl Send for Description {}
// SAFETY: Shared access exposes only immutable payload and thread-safe reference operations.
unsafe impl Sync for Description {}

// SAFETY: Native get/put maintain the initialized description allocation.
unsafe impl AlwaysRefCounted for Description {
    fn inc_ref(&self) {
        // SAFETY: The shared reference proves the native allocation remains live.
        unsafe { bindings::drm_constraints_description_get(self.0.get()) };
    }

    unsafe fn dec_ref(ptr: NonNull<Self>) {
        // SAFETY: The caller transfers a reference to the identically represented native object.
        unsafe { bindings::drm_constraints_description_put(ptr.as_ptr().cast()) };
    }
}

impl Description {
    /// Validate and copy bounded allocation metadata into native owned storage.
    ///
    /// Rejects empty or over-limit format lists, invalid dimensions, unknown fourcc values,
    /// invalid modifiers and duplicate plane/format/modifier tuples. Arbitrary supported tiled
    /// modifiers are not restricted to the kernel compositor's linear layouts.
    pub fn new(output: Size, formats: &[Format]) -> Result<ARef<Self>> {
        let count = formats.len().try_into().map_err(|_| E2BIG)?;
        // SAFETY: Transparent wrappers have native layout, both inputs remain readable for the
        // call, and native construction copies all input without retaining borrowed pointers.
        let raw = from_err_ptr(unsafe {
            bindings::drm_constraints_description_create(&output.0, formats.as_ptr().cast(), count)
        })?;
        // SAFETY: Successful construction transfers one non-null initialized native reference.
        Ok(unsafe { ARef::from_raw(NonNull::new_unchecked(raw.cast())) })
    }

    /// Borrow immutable output dimension bounds for the lifetime of this reference.
    pub fn output(&self) -> &Size {
        // SAFETY: A live description owns an initialized immutable output record. Size has its
        // native representation, and the returned borrow cannot outlive this description.
        unsafe { &*bindings::drm_constraints_description_output(self.0.get()).cast() }
    }

    /// Borrow immutable per-plane allocation records for the lifetime of this reference.
    pub fn formats(&self) -> &[Format] {
        let mut count = 0;
        // SAFETY: The description is live and count is writable. Native code returns its bounded,
        // initialized array; each Format has the same representation as one native record.
        unsafe {
            let ptr = bindings::drm_constraints_description_formats(self.0.get(), &mut count);
            slice::from_raw_parts(ptr.cast(), count as usize)
        }
    }
}

#[cfg(CONFIG_KUNIT)]
mod tests;
