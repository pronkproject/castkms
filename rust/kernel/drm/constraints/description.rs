// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Referenced allocation and scalar property metadata owned by common DRM code.

use super::Property;
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
    marker::PhantomData,
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
    const DEFAULT_STORAGE: u32 = bindings::DRM_CONSTRAINTS_FORMAT_STORAGE_NATIVE
        | bindings::DRM_CONSTRAINTS_FORMAT_STORAGE_IMPORTED;

    /// Construct explicit-layout metadata using standard DRM fourcc and modifier values.
    pub const fn new(plane_id: u32, format: u32, modifier: u64, size: Size) -> Self {
        Self(bindings::drm_constraints_format {
            plane_id,
            format,
            modifier,
            size: size.0,
            flags: 0,
            storage_flags: Self::DEFAULT_STORAGE,
            pitch_alignment: 1,
            offset_alignment: 1,
            max_pitch: u32::MAX,
        })
    }

    /// Describe framebuffer creation without an explicit modifier, not necessarily linear.
    pub const fn implicit(plane_id: u32, format: u32, size: Size) -> Self {
        Self(bindings::drm_constraints_format {
            plane_id,
            format,
            modifier: 0,
            size: size.0,
            flags: bindings::DRM_CONSTRAINTS_FORMAT_IMPLICIT,
            storage_flags: Self::DEFAULT_STORAGE,
            pitch_alignment: 1,
            offset_alignment: 1,
            max_pitch: u32::MAX,
        })
    }

    /// Restrict storage origin, pitch alignment, offset alignment and maximum pitch.
    ///
    /// At least one origin must be allowed. Alignments apply to every framebuffer memory plane
    /// and must be nonzero powers of two. [`Description::new`] validates these requirements.
    pub const fn with_storage(
        mut self,
        native: bool,
        imported: bool,
        pitch_alignment: u32,
        offset_alignment: u32,
        max_pitch: u32,
    ) -> Self {
        self.0.storage_flags = if native {
            bindings::DRM_CONSTRAINTS_FORMAT_STORAGE_NATIVE
        } else {
            0
        } | if imported {
            bindings::DRM_CONSTRAINTS_FORMAT_STORAGE_IMPORTED
        } else {
            0
        };
        self.0.pitch_alignment = pitch_alignment;
        self.0.offset_alignment = offset_alignment;
        self.0.max_pitch = max_pitch;
        self
    }

    /// DRM plane object ID.
    pub const fn plane_id(&self) -> u32 {
        self.0.plane_id
    }

    /// DRM fourcc.
    pub const fn format(&self) -> u32 {
        self.0.format
    }

    /// Explicit DRM format modifier, or `None` for implicit framebuffer layout.
    pub const fn modifier(&self) -> Option<u64> {
        if self.0.flags & bindings::DRM_CONSTRAINTS_FORMAT_IMPLICIT != 0 {
            None
        } else {
            Some(self.0.modifier)
        }
    }

    /// Inclusive framebuffer allocation bounds, not fractional source-rectangle bounds.
    pub const fn size(&self) -> Size {
        Size(self.0.size)
    }

    /// Whether storage created on the queried DRM device is permitted.
    pub const fn permits_native(&self) -> bool {
        self.0.storage_flags & bindings::DRM_CONSTRAINTS_FORMAT_STORAGE_NATIVE != 0
    }

    /// Whether PRIME-imported DMA-BUF storage is permitted.
    pub const fn permits_imported(&self) -> bool {
        self.0.storage_flags & bindings::DRM_CONSTRAINTS_FORMAT_STORAGE_IMPORTED != 0
    }

    /// Required pitch alignment, required offset alignment and maximum pitch in bytes.
    pub const fn storage_layout(&self) -> (u32, u32, u32) {
        (
            self.0.pitch_alignment,
            self.0.offset_alignment,
            self.0.max_pitch,
        )
    }
}

/// One overlapping maximum for simultaneously used KMS planes.
///
/// The description copies the borrowed plane IDs. Every supplied limit applies, allowing an
/// overall scene ceiling and narrower resource-group ceilings to coexist.
#[repr(transparent)]
pub struct PlaneLimit<'a> {
    inner: bindings::drm_constraints_plane_limit,
    _plane_ids: PhantomData<&'a [u32]>,
}

impl<'a> PlaneLimit<'a> {
    /// Describe a nonempty group of distinct plane IDs and its positive active ceiling.
    pub fn new(max_active: u32, plane_ids: &'a [u32]) -> Result<Self> {
        if plane_ids.len() > u32::MAX as usize {
            return Err(E2BIG);
        }
        Ok(Self {
            inner: bindings::drm_constraints_plane_limit {
                max_active,
                count: plane_ids.len() as u32,
                plane_ids: plane_ids.as_ptr(),
            },
            _plane_ids: PhantomData,
        })
    }

    /// Maximum number of simultaneously used planes from this group.
    pub const fn max_active(&self) -> u32 {
        self.inner.max_active
    }

    /// Borrow the input plane IDs for this construction record.
    pub fn plane_ids(&self) -> &'a [u32] {
        // SAFETY: Construction records the borrowed slice and the lifetime prevents its removal.
        unsafe { slice::from_raw_parts(self.inner.plane_ids, self.inner.count as usize) }
    }
}

/// Independently referenced, immutable allocation and scalar property description.
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
    /// Validate and copy bounded allocation metadata and scalar rules into native owned storage.
    ///
    /// Rejects empty or over-limit format lists, invalid dimensions, unknown fourcc values,
    /// invalid modifiers, malformed storage rules and duplicate plane/format/modifier tuples.
    /// Arbitrary supported tiled modifiers are not restricted to the kernel compositor's linear
    /// layouts.
    /// Scalar rules must be valid and distinct by object/property ID; an empty rule list is valid.
    pub fn new(output: Size, formats: &[Format], properties: &[Property]) -> Result<ARef<Self>> {
        Self::new_with_plane_limits(output, formats, properties, &[])
    }

    /// Validate and copy allocation metadata, scalar rules and active-plane ceilings.
    pub fn new_with_plane_limits(
        output: Size,
        formats: &[Format],
        properties: &[Property],
        plane_limits: &[PlaneLimit<'_>],
    ) -> Result<ARef<Self>> {
        let count = formats.len().try_into().map_err(|_| E2BIG)?;
        let property_count = properties.len().try_into().map_err(|_| E2BIG)?;
        let plane_limit_count = plane_limits.len().try_into().map_err(|_| E2BIG)?;
        // SAFETY: Transparent wrappers have native layout, all inputs remain readable for the
        // call, and native construction copies all input without retaining borrowed pointers.
        let raw = from_err_ptr(unsafe {
            bindings::drm_constraints_description_create_with_plane_limits(
                &output.0,
                formats.as_ptr().cast(),
                count,
                properties.as_ptr().cast(),
                property_count,
                plane_limits.as_ptr().cast(),
                plane_limit_count,
            )
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

    /// Borrow immutable scalar property rules. An empty slice imposes no additional scalar rules.
    pub fn properties(&self) -> &[Property] {
        let mut count = 0;
        // SAFETY: The description remains live and count is writable. Native code returns its
        // bounded immutable property array, or NULL with count zero.
        let ptr =
            unsafe { bindings::drm_constraints_description_properties(self.0.get(), &mut count) };
        if count == 0 {
            return &[];
        }
        // SAFETY: Nonempty native arrays are initialized, non-null and retained by this borrow.
        // Property transparently represents one native record.
        unsafe { slice::from_raw_parts(ptr.cast(), count as usize) }
    }

    /// Borrow immutable overlapping active-plane ceilings.
    pub fn plane_limits(&self) -> &[PlaneLimit<'_>] {
        let mut count = 0;
        // SAFETY: The description owns the bounded record array and every nested ID array. The
        // returned records and their ID views cannot outlive this description borrow.
        let ptr = unsafe {
            bindings::drm_constraints_description_plane_limits(self.0.get(), &mut count)
        };
        if count == 0 {
            return &[];
        }
        // SAFETY: A nonempty native array is initialized, non-null and retained by the borrow.
        unsafe { slice::from_raw_parts(ptr.cast(), count as usize) }
    }
}

#[cfg(CONFIG_KUNIT)]
mod tests;
