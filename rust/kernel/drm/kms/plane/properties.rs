// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Identities of attached standard plane scene properties.

use crate::bindings;

/// Standard scalar plane properties used to describe scene restrictions.
///
/// A name identifies the native property, not its current value or whether a particular
/// restriction is valid. Optional properties may be absent; immutable properties cannot
/// receive constraints rules. Native output validation checks attachment, type and value range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SceneProperty {
    /// Destination horizontal position in pixels.
    CrtcX,
    /// Destination vertical position in pixels.
    CrtcY,
    /// Destination width in pixels.
    CrtcWidth,
    /// Destination height in pixels.
    CrtcHeight,
    /// Source horizontal position in unsigned 16.16 pixels.
    SourceX,
    /// Source vertical position in unsigned 16.16 pixels.
    SourceY,
    /// Source width in unsigned 16.16 pixels.
    SourceWidth,
    /// Source height in unsigned 16.16 pixels.
    SourceHeight,
    /// Constant plane alpha.
    Alpha,
    /// Interpretation of pixel alpha.
    BlendMode,
    /// Rotation and reflection bitmask.
    Rotation,
    /// Plane stacking position.
    Zpos,
    /// YUV color encoding enum.
    ColorEncoding,
    /// YUV color range enum.
    ColorRange,
    /// Scaling filter enum.
    ScalingFilter,
    /// Cursor horizontal hotspot.
    HotspotX,
    /// Cursor vertical hotspot.
    HotspotY,
}

// The caller retains an initialized plane and excludes concurrent property attachment/cleanup.
pub(super) unsafe fn id(plane: *mut bindings::drm_plane, name: SceneProperty) -> Option<u32> {
    // SAFETY: The caller stabilizes the device and property slots. Read only those fields,
    // without borrowing whole native objects containing concurrently mutable display state.
    let property = unsafe {
        let config = &raw const (*(*plane).dev).mode_config;
        match name {
            SceneProperty::CrtcX => (*config).prop_crtc_x,
            SceneProperty::CrtcY => (*config).prop_crtc_y,
            SceneProperty::CrtcWidth => (*config).prop_crtc_w,
            SceneProperty::CrtcHeight => (*config).prop_crtc_h,
            SceneProperty::SourceX => (*config).prop_src_x,
            SceneProperty::SourceY => (*config).prop_src_y,
            SceneProperty::SourceWidth => (*config).prop_src_w,
            SceneProperty::SourceHeight => (*config).prop_src_h,
            SceneProperty::Alpha => (*plane).alpha_property,
            SceneProperty::BlendMode => (*plane).blend_mode_property,
            SceneProperty::Rotation => (*plane).rotation_property,
            SceneProperty::Zpos => (*plane).zpos_property,
            SceneProperty::ColorEncoding => (*plane).color_encoding_property,
            SceneProperty::ColorRange => (*plane).color_range_property,
            SceneProperty::ScalingFilter => (*plane).scaling_filter_property,
            SceneProperty::HotspotX => (*plane).hotspot_x_property,
            SceneProperty::HotspotY => (*plane).hotspot_y_property,
        }
    };
    if property.is_null() {
        return None;
    }
    // SAFETY: The object owns this initialized array; attachment and cleanup are excluded.
    let attached = unsafe { (*plane).base.properties };
    if attached.is_null() {
        return None;
    }
    // SAFETY: Property identities and count are stable during the caller's borrow. Borrow only
    // the pointer array, not the separate property values which native state updates may change.
    let (properties, count) = unsafe { (&(*attached).properties, (*attached).count as usize) };
    if !properties
        .iter()
        .take(count)
        .any(|&candidate| candidate == property)
    {
        return None;
    }
    // SAFETY: A matching attached property is retained by mode configuration until cleanup.
    Some(unsafe { (*property).base.id })
}
