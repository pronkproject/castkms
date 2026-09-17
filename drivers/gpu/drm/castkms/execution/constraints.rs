// SPDX-License-Identifier: GPL-2.0-only

//! Generic allocation descriptions derived from renderer constraints.

pub(crate) mod bindings;
pub(crate) mod backend;
pub(crate) mod provider;

use super::{
    capabilities::Profile,
    potential, //
};
use crate::scene::Kind;
use kernel::{
    bindings as kernel_bindings,
    drm::{
        constraints::{
            Description,
            Format,
            PlaneGeometry,
            PlaneLimit as ActivePlaneLimit,
            Property,
            Size, //
        },
        fourcc,
        kms::plane::{RawPlane, SceneProperty}, //
    },
    prelude::*,
    sync::aref::ARef, //
};

const _: () = assert!(
    crate::execution::capabilities::MAX_FORMATS * crate::scene::MAX_PLANES
        <= kernel_bindings::DRM_CONSTRAINTS_MAX_FORMATS as usize
);
const _: () = assert!(
    crate::scene::MAX_PLANES <= kernel_bindings::DRM_CONSTRAINTS_MAX_PLANE_GEOMETRIES as usize
);

/// Existing plane identity and role; native publication validates output membership.
pub(crate) struct Plane {
    pub(crate) id: u32,
    pub(crate) kind: Kind,
    properties: PlaneProperties,
}

#[derive(Clone, Copy)]
struct PlaneProperties {
    color_encoding: u32,
    color_range: u32,
}

impl PlaneProperties {
    fn ids(self) -> [u32; 2] {
        [self.color_encoding, self.color_range]
    }
}

impl Plane {
    pub(crate) fn from_kms(plane: &impl RawPlane, kind: Kind) -> Result<Self> {
        let required = |property| plane.scene_property_id(property).ok_or(EOPNOTSUPP);
        Ok(Self {
            id: plane.object_id(),
            kind,
            properties: PlaneProperties {
                color_encoding: required(SceneProperty::ColorEncoding)?,
                color_range: required(SceneProperty::ColorRange)?,
            },
        })
    }
}

/// Owned immutable plane and property identities for one output's descriptions.
///
/// The setup owner supplies existing planes eligible for that output. Numeric identities retain
/// neither KMS objects nor authority; native publication still validates attachment and scope.
pub(crate) struct Topology {
    planes: KVec<Plane>,
}

impl Topology {
    pub(crate) fn new(planes: KVec<Plane>) -> Result<Self> {
        check_planes(&planes)?;
        Ok(Self { planes })
    }

    pub(crate) fn planes(&self) -> &[Plane] {
        &self.planes
    }
}

fn check_planes(planes: &[Plane]) -> Result {
    if planes.is_empty() || planes.len() > crate::scene::MAX_PLANES {
        return Err(EINVAL);
    }
    for (index, plane) in planes.iter().enumerate() {
        let properties = plane.properties.ids();
        if plane.id == 0
            || properties.contains(&0)
            || properties
                .iter()
                .enumerate()
                .any(|(property, id)| properties[..property].contains(id))
            || planes[..index]
                .iter()
                .any(|previous| previous.id == plane.id)
        {
            return Err(EINVAL);
        }
    }
    Ok(())
}

fn role(plane: &Plane) -> usize {
    match plane.kind {
        Kind::Primary => 0,
        Kind::Overlay => 1,
        Kind::Cursor => 2,
    }
}

fn format_supported(
    plane: &Plane,
    format: &super::capabilities::Format,
    minimum_width: u32,
) -> bool {
    potential::FORMATS.contains(&format.fourcc)
        && (plane.kind != Kind::Cursor || format.fourcc == fourcc::ARGB8888)
        && format.planes as usize == potential::plane_count(format.fourcc)
        && (0..format.planes as usize).all(|memory_plane| {
            fourcc::minimum_pitch(format.fourcc, memory_plane, minimum_width).is_some_and(
                |minimum| {
                    let alignment = u64::from(format.pitch_alignment);
                    minimum
                        .max(1)
                        .checked_add(alignment - 1)
                        .map(|pitch| pitch & !(alignment - 1))
                        .is_some_and(|pitch| pitch <= u64::from(format.max_pitch))
                },
            )
        })
}

fn source_ceiling(plane: &Plane) -> u32 {
    if plane.kind == Kind::Cursor {
        potential::MAX_CURSOR_DIMENSION
    } else {
        potential::MAX_DIMENSION
    }
}

fn plane_supported(profile: &Profile, plane: &Plane) -> bool {
    let limits = profile.limits();
    limits.roles[role(plane)] != 0
        && bounds(
            limits.geometry.min_source,
            limits.geometry.source,
            source_ceiling(plane),
        )
        .is_some()
        && profile
            .formats()
            .iter()
            .any(|format| format_supported(plane, format, limits.geometry.min_source[0]))
}

fn enabled_mask<const N: usize>(enabled: &[bool; N], values: [u32; N]) -> u64 {
    enabled
        .iter()
        .zip(values)
        .fold(0, |mask, (enabled, value)| {
            mask | if *enabled { 1 << value } else { 0 }
        })
}

fn renderer_properties(profile: &Profile, planes: &[Plane]) -> Result<KVec<Property>> {
    let limits = profile.limits();
    let encoding_mask = enabled_mask(
        &limits.color.yuv_encodings,
        [
            kernel_bindings::drm_color_encoding_DRM_COLOR_YCBCR_BT601,
            kernel_bindings::drm_color_encoding_DRM_COLOR_YCBCR_BT709,
            kernel_bindings::drm_color_encoding_DRM_COLOR_YCBCR_BT2020,
        ],
    );
    let range_mask = enabled_mask(
        &limits.color.yuv_ranges,
        [
            kernel_bindings::drm_color_range_DRM_COLOR_YCBCR_LIMITED_RANGE,
            kernel_bindings::drm_color_range_DRM_COLOR_YCBCR_FULL_RANGE,
        ],
    );
    let mut properties = KVec::new();
    for plane in planes {
        if !plane_supported(profile, plane) {
            continue;
        }
        let ids = plane.properties;
        let supports_yuv = profile
            .formats()
            .iter()
            .any(|format| {
                format_supported(plane, format, limits.geometry.min_source[0])
                    && crate::formats::is_yuv(format.fourcc)
            });
        if supports_yuv {
            properties.push(
                Property::enum_values(plane.id, ids.color_encoding, encoding_mask)
                    .for_yuv_plane(),
                GFP_KERNEL,
            )?;
            properties.push(
                Property::enum_values(plane.id, ids.color_range, range_mask).for_yuv_plane(),
                GFP_KERNEL,
            )?;
        }
    }
    Ok(properties)
}

fn renderer_plane_groups(
    profile: &Profile,
    planes: &[Plane],
) -> Result<(KVec<u32>, [KVec<u32>; 3])> {
    let mut all = KVec::new();
    let mut roles: [KVec<u32>; 3] = core::array::from_fn(|_| KVec::new());
    for plane in planes {
        if plane_supported(profile, plane) {
            all.push(plane.id, GFP_KERNEL)?;
            roles[role(plane)].push(plane.id, GFP_KERNEL)?;
        }
    }
    Ok((all, roles))
}

fn bounds(minimum: [u32; 2], maximum: [u32; 2], ceiling: u32) -> Option<Size> {
    let maximum = [maximum[0].min(ceiling), maximum[1].min(ceiling)];
    if minimum[0] > maximum[0] || minimum[1] > maximum[1] {
        return None;
    }
    Some(Size::new(minimum[0], minimum[1], maximum[0], maximum[1]))
}

fn host_geometry(plane: &Plane) -> PlaneGeometry {
    PlaneGeometry::new(plane.id, true, true, true, 1 << 12, 1 << 20)
}

fn renderer_geometry(profile: &Profile, plane: &Plane) -> PlaneGeometry {
    let geometry = profile.limits().geometry;
    PlaneGeometry::new(
        plane.id,
        geometry.crop,
        geometry.fractional,
        geometry.position,
        geometry.min_scale,
        geometry.max_scale,
    )
}

/// Describe allocations for the built-in compositor on the supplied existing planes.
///
/// Both implicit layout and explicit linear layout are accepted. The framebuffer validator
/// remains responsible for storage bounds, and complete-scene checks validate operations.
/// The caller supplies standard scalar property rules; no readiness or source access is granted.
pub(crate) fn host(planes: &[Plane], properties: &[Property]) -> Result<ARef<Description>> {
    check_planes(planes)?;
    let output = Size::new(1, 1, super::host::MAX_WIDTH, super::host::MAX_HEIGHT);
    let mut formats = KVec::new();
    let mut geometries = KVec::new();
    for plane in planes {
        let size = if plane.kind == Kind::Cursor {
            Size::new(
                1,
                1,
                super::host::MAX_WIDTH.min(potential::MAX_CURSOR_DIMENSION),
                super::host::MAX_HEIGHT.min(potential::MAX_CURSOR_DIMENSION),
            )
        } else {
            output
        };
        for &format in super::host::FORMATS {
            if plane.kind == Kind::Cursor && format != fourcc::ARGB8888 {
                continue;
            }
            formats.push(Format::implicit(plane.id, format, size), GFP_KERNEL)?;
            formats.push(
                Format::new(plane.id, format, fourcc::FORMAT_MOD_LINEAR, size),
                GFP_KERNEL,
            )?;
        }
        geometries.push(host_geometry(plane), GFP_KERNEL)?;
    }
    Description::new_with_geometry(output, &formats, properties, &[], &geometries)
}

/// Describe the profile's allocation choices within the fixed KMS object envelope.
///
/// Geometry is intersected with native allocation limits, including the cursor limit.
/// Unsupported roles and formats contribute no allocations. An empty intersection fails;
/// it does not become unrestricted. Per-plane geometry and representable scalar restrictions are
/// derived from the profile; color pipelines and other whole-scene limits remain subject to final
/// validation.
/// Native publication still checks object membership, and the retained profile must validate
/// complete scenes, layer counts and color operations at acceptance.
/// This operation allocates metadata only; it neither establishes readiness nor grants access.
pub(crate) fn renderer(profile: &Profile, planes: &[Plane]) -> Result<ARef<Description>> {
    check_planes(planes)?;
    let limits = profile.limits();
    let output = bounds(
        limits.geometry.min_output,
        limits.geometry.output,
        potential::MAX_DIMENSION,
    )
    .ok_or(EOPNOTSUPP)?;
    let mut formats = KVec::new();
    let mut geometries = KVec::new();
    for plane in planes {
        if !plane_supported(profile, plane) {
            continue;
        }
        let size = bounds(
            limits.geometry.min_source,
            limits.geometry.source,
            source_ceiling(plane),
        )
        .ok_or(EOPNOTSUPP)?;
        geometries.push(renderer_geometry(profile, plane), GFP_KERNEL)?;
        for format in profile.formats() {
            if !format_supported(plane, format, limits.geometry.min_source[0]) {
                continue;
            }
            let format = match format.modifier {
                Some(modifier) => Format::new(plane.id, format.fourcc, modifier, size),
                None => Format::implicit(plane.id, format.fourcc, size),
            }
            .with_storage(
                format.native,
                format.imported,
                format.pitch_alignment,
                format.offset_alignment,
                format.max_pitch,
            );
            formats.push(format, GFP_KERNEL)?;
        }
    }
    if formats.is_empty() {
        return Err(EOPNOTSUPP);
    }
    let (all, role_groups) = renderer_plane_groups(profile, planes)?;
    let mut plane_limits = KVec::new();
    if limits.layers < all.len() {
        plane_limits.push(ActivePlaneLimit::new(limits.layers as u32, &all)?, GFP_KERNEL)?;
    }
    for (group, maximum) in role_groups.iter().zip(limits.roles) {
        if maximum < group.len() {
            plane_limits.push(ActivePlaneLimit::new(maximum as u32, group)?, GFP_KERNEL)?;
        }
    }
    Description::new_with_geometry(
        output,
        &formats,
        &renderer_properties(profile, planes)?,
        &plane_limits,
        &geometries,
    )
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
#[kunit_tests(rust_castkms_constraints_description)]
mod tests {
    use super::*;
    use crate::execution::capabilities::{
        ColorLimits,
        Format as RendererFormat,
        GeometryLimits,
        Limits, //
    };

    const TILED: u64 = (1 << 56) | 1;

    fn limits() -> Limits {
        Limits {
            geometry: GeometryLimits {
                min_output: [1920, 1080],
                output: [1920, 1080],
                min_source: [64, 32],
                source: [8192, 8192],
                crop: true,
                fractional: true,
                position: true,
                scale: true,
                min_scale: 1 << 12,
                max_scale: 1 << 20,
            },
            color: ColorLimits {
                operations: 16,
                srgb: true,
                plane_matrix: true,
                output_matrix: true,
                lut_entries: 256,
                yuv_encodings: [true; 3],
                yuv_ranges: [true; 2],
            },
            layers: 4,
            roles: [1, 2, 1],
        }
    }

    fn profile(limits: Limits) -> Result<Profile> {
        let mut formats = KVec::new();
        for (fourcc, modifier) in [
            (fourcc::XRGB8888, Some(TILED)),
            (fourcc::XRGB8888, None),
            (fourcc::ARGB8888, Some(fourcc::FORMAT_MOD_LINEAR)),
        ] {
            formats.push(
                RendererFormat {
                    fourcc,
                    modifier,
                    planes: 1,
                    native: true,
                    imported: true,
                    pitch_alignment: 4,
                    offset_alignment: 4,
                    max_pitch: 65536,
                },
                GFP_KERNEL,
            )?;
        }
        Profile::new(limits, formats)
    }

    fn planes() -> [Plane; 3] {
        [
            Plane {
                id: 7,
                kind: Kind::Primary,
                properties: PlaneProperties {
                    color_encoding: 21,
                    color_range: 22,
                },
            },
            Plane {
                id: 8,
                kind: Kind::Overlay,
                properties: PlaneProperties {
                    color_encoding: 23,
                    color_range: 24,
                },
            },
            Plane {
                id: 9,
                kind: Kind::Cursor,
                properties: PlaneProperties {
                    color_encoding: 25,
                    color_range: 26,
                },
            },
        ]
    }

    #[test]
    fn exact_output_retains_tiled_and_implicit_source_choices() -> Result {
        let profile = profile(limits())?;
        let description = renderer(&profile, &planes())?;
        assert_eq!(description.output().minimum(), (1920, 1080));
        assert_eq!(description.output().maximum(), (1920, 1080));
        let formats = description.formats();
        assert_eq!(formats.len(), 7);
        assert_eq!(formats[0].modifier(), Some(TILED));
        assert_eq!(formats[1].modifier(), None);
        assert_eq!(formats[0].size().minimum(), (64, 32));
        assert_eq!(formats[0].size().maximum(), (8192, 8192));
        assert_eq!(formats[3].plane_id(), 8);
        assert_eq!(formats[6].plane_id(), 9);
        assert_eq!(formats[6].format(), fourcc::ARGB8888);
        assert_eq!(formats[6].size().maximum(), (512, 512));
        assert!(description.properties().is_empty());
        assert!(description.plane_limits().is_empty());
        let geometries = description.plane_geometries();
        assert_eq!(geometries.len(), 3);
        assert_eq!(geometries[0].plane_id(), 7);
        assert_eq!(geometries[0].operations(), (true, true, true));
        assert_eq!(geometries[0].scale(), (1 << 12, 1 << 20));
        Ok(())
    }

    #[test]
    fn profile_restrictions_become_geometry_and_color_rules() -> Result {
        let mut limits = limits();
        limits.geometry.position = false;
        limits.geometry.crop = false;
        limits.geometry.fractional = false;
        limits.geometry.scale = false;
        limits.geometry.min_scale = 1 << 16;
        limits.geometry.max_scale = 1 << 16;
        limits.color.yuv_encodings = [false, true, false];
        limits.color.yuv_ranges = [false, true];
        limits.roles = [1, 0, 0];
        let mut formats = KVec::new();
        formats.push(
            RendererFormat {
                fourcc: fourcc::NV12,
                modifier: Some(TILED),
                planes: 2,
                native: false,
                imported: true,
                pitch_alignment: 16,
                offset_alignment: 4096,
                max_pitch: 65536,
            },
            GFP_KERNEL,
        )?;
        let profile = Profile::new(limits, formats)?;
        let description = renderer(&profile, &planes())?;
        let properties = description.properties();
        assert_eq!(properties.len(), 2);
        assert!(properties.iter().all(|property| property.object_id() == 7));
        for (property, expected) in properties.iter().zip([21, 22]) {
            assert_eq!(property.property_id(), expected);
        }
        assert_eq!(
            properties[0].mask(),
            1 << kernel_bindings::drm_color_encoding_DRM_COLOR_YCBCR_BT709
        );
        assert_eq!(
            properties[1].mask(),
            1 << kernel_bindings::drm_color_range_DRM_COLOR_YCBCR_FULL_RANGE
        );
        assert!(properties.iter().all(|property| {
            property.applicability_flags()
                == kernel_bindings::DRM_MODE_CONSTRAINTS_PROPERTY_PLANE_YUV
        }));
        let geometries = description.plane_geometries();
        assert_eq!(geometries.len(), 1);
        assert_eq!(geometries[0].plane_id(), 7);
        assert_eq!(geometries[0].operations(), (false, false, false));
        assert_eq!(geometries[0].scale(), (1 << 16, 1 << 16));
        Ok(())
    }

    #[test]
    fn mixed_rgb_and_yuv_allocations_keep_color_rules_conditional() -> Result {
        let mut limits = limits();
        limits.color.yuv_encodings = [false, true, false];
        limits.color.yuv_ranges = [false, true];
        limits.roles = [1, 0, 0];
        let mut formats = KVec::new();
        for (fourcc, planes) in [(fourcc::XRGB8888, 1), (fourcc::NV12, 2)] {
            formats.push(
                RendererFormat {
                    fourcc,
                    modifier: Some(TILED),
                    planes,
                    native: false,
                    imported: true,
                    pitch_alignment: 16,
                    offset_alignment: 4096,
                    max_pitch: 65536,
                },
                GFP_KERNEL,
            )?;
        }

        let description = renderer(&Profile::new(limits, formats)?, &planes())?;
        assert_eq!(description.properties().len(), 2);
        assert!(description.properties().iter().all(|property| {
            property.applicability_flags()
                == kernel_bindings::DRM_MODE_CONSTRAINTS_PROPERTY_PLANE_YUV
        }));
        assert_eq!(description.formats().len(), 2);
        Ok(())
    }

    #[test]
    fn formats_require_a_constructible_pitch_at_minimum_width() -> Result {
        let mut limits = limits();
        limits.geometry.min_source = [1920, 1080];
        limits.geometry.source = [1920, 1080];
        limits.roles = [1, 0, 0];
        let mut formats = KVec::new();
        formats.push(
            RendererFormat {
                fourcc: fourcc::XRGB8888,
                modifier: Some(TILED),
                planes: 1,
                native: false,
                imported: true,
                pitch_alignment: 256,
                offset_alignment: 4096,
                max_pitch: 7679,
            },
            GFP_KERNEL,
        )?;
        let profile = Profile::new(limits, formats)?;
        assert!(matches!(renderer(&profile, &planes()), Err(EOPNOTSUPP)));

        let mut formats = KVec::new();
        formats.push(
            RendererFormat {
                max_pitch: 7680,
                ..profile.formats()[0]
            },
            GFP_KERNEL,
        )?;
        let description = renderer(&Profile::new(limits, formats)?, &planes())?;
        assert_eq!(description.formats().len(), 1);
        assert_eq!(description.formats()[0].storage_layout().2, 7680);
        Ok(())
    }

    #[test]
    fn impossible_rgb_tuple_does_not_hide_yuv_property_rules() -> Result {
        let mut limits = limits();
        limits.geometry.min_source = [1920, 1080];
        limits.geometry.source = [1920, 1080];
        limits.color.yuv_encodings = [false, true, false];
        limits.color.yuv_ranges = [false, true];
        limits.roles = [1, 0, 0];
        let mut formats = KVec::new();
        for (fourcc, planes, max_pitch) in [
            (fourcc::XRGB8888, 1, 4096),
            (fourcc::NV12, 2, 2048),
        ] {
            formats.push(
                RendererFormat {
                    fourcc,
                    modifier: Some(TILED),
                    planes,
                    native: false,
                    imported: true,
                    pitch_alignment: 256,
                    offset_alignment: 4096,
                    max_pitch,
                },
                GFP_KERNEL,
            )?;
        }

        let description = renderer(&Profile::new(limits, formats)?, &planes())?;
        assert_eq!(description.formats().len(), 1);
        assert_eq!(description.formats()[0].format(), fourcc::NV12);
        assert_eq!(description.properties().len(), 2);
        Ok(())
    }

    #[test]
    fn layer_and_role_counts_become_overlapping_plane_limits() -> Result {
        let mut topology = KVec::new();
        for plane in planes() {
            topology.push(plane, GFP_KERNEL)?;
        }
        for (id, color_encoding) in [(10, 27), (11, 29)] {
            topology.push(
                Plane {
                    id,
                    kind: Kind::Overlay,
                    properties: PlaneProperties {
                        color_encoding,
                        color_range: color_encoding + 1,
                    },
                },
                GFP_KERNEL,
            )?;
        }
        let mut limits = limits();
        limits.layers = 3;
        limits.roles = [1, 2, 1];
        let profile = profile(limits)?;
        let description = renderer(&profile, &topology)?;
        let plane_limits = description.plane_limits();
        assert_eq!(plane_limits.len(), 2);
        assert_eq!(plane_limits[0].max_active(), 3);
        assert_eq!(plane_limits[0].plane_ids(), [7, 8, 9, 10, 11]);
        assert_eq!(plane_limits[1].max_active(), 2);
        assert_eq!(plane_limits[1].plane_ids(), [8, 10, 11]);
        Ok(())
    }

    #[test]
    fn real_descriptions_preserve_directional_contract_coverage() -> Result {
        let mut restricted = limits();
        restricted.geometry.source = [1920, 1080];
        restricted.geometry.crop = false;
        restricted.geometry.position = false;
        restricted.layers = 1;
        restricted.roles = [1, 1, 1];

        let broad = renderer(&profile(limits())?, &planes())?;
        let narrow = renderer(&profile(restricted)?, &planes())?;
        let host = host(&planes(), &[])?;

        assert!(broad.covers(&narrow));
        assert!(!narrow.covers(&broad));
        assert!(!host.covers(&narrow));
        assert!(host.covers(&host));
        Ok(())
    }

    #[test]
    fn unsupported_roles_do_not_advertise_allocations() -> Result {
        let mut limits = limits();
        limits.roles = [1, 0, 0];
        let description = renderer(&profile(limits)?, &planes())?;
        assert_eq!(description.formats().len(), 3);
        assert!(description
            .formats()
            .iter()
            .all(|format| format.plane_id() == 7));
        limits.roles = [0, 0, 1];
        limits.geometry.min_source = [1024; 2];
        assert!(matches!(
            renderer(&profile(limits)?, &planes()),
            Err(EOPNOTSUPP)
        ));
        Ok(())
    }

    #[test]
    fn allocation_intersection_never_widens_exact_geometry() -> Result {
        let mut limits = limits();
        limits.geometry.output = [32768; 2];
        let description = renderer(&profile(limits)?, &planes())?;
        assert_eq!(description.output().maximum(), (16384, 16384));
        limits.geometry.min_output = [32768; 2];
        assert!(matches!(
            renderer(&profile(limits)?, &planes()),
            Err(EOPNOTSUPP)
        ));
        Ok(())
    }

    #[test]
    fn floating_point_tiled_formats_do_not_inherit_host_limits() -> Result {
        let mut formats = KVec::new();
        for planes in [1, 2] {
            formats.push(
                RendererFormat {
                    fourcc: fourcc::XRGB16161616F,
                    modifier: Some(TILED),
                    planes,
                    native: false,
                    imported: true,
                    pitch_alignment: 16,
                    offset_alignment: 16,
                    max_pitch: 131072,
                },
                GFP_KERNEL,
            )?;
        }
        let mut limits = limits();
        limits.geometry.min_output = [5120, 2880];
        limits.geometry.output = [5120, 2880];
        let profile = Profile::new(limits, formats)?;
        let description = renderer(&profile, &planes()[..1])?;
        assert_eq!(description.output().minimum(), (5120, 2880));
        assert_eq!(description.output().maximum(), (5120, 2880));
        assert_eq!(description.formats().len(), 1);
        assert_eq!(description.formats()[0].format(), fourcc::XRGB16161616F);
        assert_eq!(description.formats()[0].modifier(), Some(TILED));
        assert!(!description.formats()[0].permits_native());
        assert!(description.formats()[0].permits_imported());
        assert_eq!(description.formats()[0].storage_layout(), (16, 16, 131072));
        Ok(())
    }

    #[test]
    fn ambiguous_topology_identity_is_rejected() -> Result {
        let profile = profile(limits())?;
        let mut planes = planes();
        assert!(matches!(renderer(&profile, &[]), Err(EINVAL)));
        planes[1].id = planes[0].id;
        assert!(matches!(renderer(&profile, &planes), Err(EINVAL)));
        planes[1].id = 0;
        assert!(matches!(renderer(&profile, &planes), Err(EINVAL)));
        planes[1].id = 8;
        planes[1].properties.color_range = planes[1].properties.color_encoding;
        assert!(matches!(renderer(&profile, &planes), Err(EINVAL)));
        planes[1].properties.color_range = 0;
        assert!(matches!(renderer(&profile, &planes), Err(EINVAL)));
        Ok(())
    }

    #[test]
    fn host_describes_only_builtin_allocation_choices() -> Result {
        let rules = [Property::unsigned_range(8, 17, 1, 30)];
        let description = host(&planes(), &rules)?;
        assert_eq!(description.output().minimum(), (1, 1));
        assert_eq!(description.output().maximum(), (8192, 8192));
        assert_eq!(description.properties()[0].bounds(), (1, 30));
        assert_eq!(description.plane_geometries().len(), 3);
        for geometry in description.plane_geometries() {
            assert_eq!(geometry.operations(), (true, true, true));
            assert_eq!(geometry.scale(), (1 << 12, 1 << 20));
        }
        let formats = description.formats();
        assert_eq!(formats.len(), 4 * super::super::host::FORMATS.len() + 2);
        for pair in formats.chunks_exact(2) {
            assert_eq!(pair[0].modifier(), None);
            assert_eq!(pair[1].modifier(), Some(fourcc::FORMAT_MOD_LINEAR));
            assert_eq!(pair[0].format(), pair[1].format());
            assert_eq!(pair[0].plane_id(), pair[1].plane_id());
            assert!(super::super::host::FORMATS.contains(&pair[0].format()));
            assert_eq!(pair[0].size().minimum(), (1, 1));
            assert_eq!(pair[0].size().minimum(), pair[1].size().minimum());
            assert_eq!(pair[0].size().maximum(), pair[1].size().maximum());
            assert!(pair[0].permits_native());
            assert!(pair[0].permits_imported());
            assert_eq!(pair[0].storage_layout(), (1, 1, u32::MAX));
            assert_eq!(pair[0].storage_layout(), pair[1].storage_layout());
            if pair[0].plane_id() == 9 {
                assert_eq!(pair[0].format(), fourcc::ARGB8888);
                assert_eq!(pair[0].size().maximum(), (512, 512));
            } else {
                assert_eq!(pair[0].size().maximum(), (8192, 8192));
            }
        }
        assert!(!formats
            .iter()
            .any(|format| format.format() == fourcc::XRGB16161616F));
        Ok(())
    }

    #[test]
    fn host_rejects_ambiguous_plane_identity() -> Result {
        assert!(matches!(host(&[], &[]), Err(EINVAL)));
        let mut planes = planes();
        planes[1].id = planes[0].id;
        assert!(matches!(host(&planes, &[]), Err(EINVAL)));
        planes[1].id = 0;
        assert!(matches!(host(&planes, &[]), Err(EINVAL)));
        Ok(())
    }

    #[test]
    fn topology_owns_validated_plane_identities() -> Result {
        assert!(matches!(Topology::new(KVec::new()), Err(EINVAL)));
        let mut stored = KVec::new();
        for plane in planes() {
            stored.push(plane, GFP_KERNEL)?;
        }
        let topology = Topology::new(stored)?;
        assert_eq!(topology.planes().len(), 3);
        assert_eq!(topology.planes()[1].id, 8);
        assert!(topology.planes()[1].kind == Kind::Overlay);
        assert_eq!(
            host(topology.planes(), &[])?.output().maximum(),
            (8192, 8192)
        );
        Ok(())
    }
}
