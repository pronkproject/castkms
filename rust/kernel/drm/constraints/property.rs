// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Scalar restrictions with standard DRM property value encodings.

/// One object's permitted values for an attached standard scalar scene property.
///
/// Construction does not validate the rule. [`super::Description::new`] checks bounded metadata;
/// native output registration checks attachment, type and the advertised value domain. Request-only
/// sentinels, driver-private properties, blobs and object references are not scalar scene rules.
#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct Property(pub(super) bindings::drm_constraints_property);

impl Property {
    const fn new(
        object_id: u32,
        property_id: u32,
        type_: u32,
        min: u64,
        max: u64,
        mask: u64,
    ) -> Self {
        Self(bindings::drm_constraints_property {
            object_id,
            property_id,
            type_,
            flags: 0,
            minimum: min,
            maximum: max,
            mask,
        })
    }

    /// Inclusive bounds for an unsigned range property.
    pub const fn unsigned_range(object_id: u32, property_id: u32, min: u64, max: u64) -> Self {
        Self::new(
            object_id,
            property_id,
            bindings::DRM_MODE_PROP_RANGE,
            min,
            max,
            0,
        )
    }

    /// Inclusive bounds for a signed range property, using DRM's two's-complement encoding.
    pub const fn signed_range(object_id: u32, property_id: u32, min: i64, max: i64) -> Self {
        Self::new(
            object_id,
            property_id,
            bindings::DRM_MODE_PROP_SIGNED_RANGE,
            min as u64,
            max as u64,
            0,
        )
    }

    /// Permitted enum values 0..63, represented by their corresponding mask bits.
    pub const fn enum_values(object_id: u32, property_id: u32, values: u64) -> Self {
        Self::new(
            object_id,
            property_id,
            bindings::DRM_MODE_PROP_ENUM,
            0,
            0,
            values,
        )
    }

    /// Permitted bits for a bitmask property; a zero mask permits only zero.
    pub const fn bitmask(object_id: u32, property_id: u32, bits: u64) -> Self {
        Self::new(
            object_id,
            property_id,
            bindings::DRM_MODE_PROP_BITMASK,
            0,
            0,
            bits,
        )
    }

    /// Apply the rule only while the named plane uses a YUV framebuffer.
    pub const fn for_yuv_plane(mut self) -> Self {
        self.0.flags = bindings::DRM_MODE_CONSTRAINTS_PROPERTY_PLANE_YUV;
        self
    }

    /// Applicability flags controlling which uses of the object receive the rule.
    pub const fn applicability_flags(&self) -> u32 {
        self.0.flags
    }

    /// CRTC or plane object ID; the rule itself does not retain the object.
    pub const fn object_id(&self) -> u32 {
        self.0.object_id
    }

    /// ID of the attached property, not a property name or independent authority.
    pub const fn property_id(&self) -> u32 {
        self.0.property_id
    }

    /// Native DRM property type: range, signed range, enum or bitmask.
    pub const fn property_type(&self) -> u32 {
        self.0.type_
    }

    /// Native inclusive bounds. Signed bounds use two's-complement u64 values.
    /// Enum and bitmask records return zero bounds.
    pub const fn bounds(&self) -> (u64, u64) {
        (self.0.minimum, self.0.maximum)
    }

    /// Permitted enum values or bitmask bits. Range records return zero.
    pub const fn mask(&self) -> u64 {
        self.0.mask
    }

    /// Check one scalar using native rule semantics, not full scene validation.
    pub fn matches(&self, value: u64) -> bool {
        // SAFETY: The initialized native record is immutably borrowed for the pure comparison.
        unsafe { bindings::drm_constraints_property_matches(&self.0, value) }
    }
}
