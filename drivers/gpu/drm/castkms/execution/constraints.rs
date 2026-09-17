// SPDX-License-Identifier: GPL-2.0-only

//! Generic allocation descriptions derived from renderer capability profiles.

pub(crate) mod bindings;

use super::{
    capabilities::Profile,
    potential, //
};
use crate::scene::Kind;
use kernel::{
    drm::{
        constraints::{
            Description,
            Format,
            Property,
            Size, //
        },
        fourcc, //
    },
    prelude::*,
    sync::aref::ARef, //
};

/// Existing plane identity and role; native publication validates output membership.
pub(crate) struct Plane {
    pub(crate) id: u32,
    pub(crate) kind: Kind,
}

/// Owned immutable plane identities for one output's allocation descriptions.
///
/// The setup owner supplies existing planes eligible for that output. Numeric identities
/// retain neither KMS objects nor authority; native publication still validates scope.
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
        if plane.id == 0
            || planes[..index]
                .iter()
                .any(|previous| previous.id == plane.id)
        {
            return Err(EINVAL);
        }
    }
    Ok(())
}

fn bounds(minimum: [u32; 2], maximum: [u32; 2], ceiling: u32) -> Option<Size> {
    let maximum = [maximum[0].min(ceiling), maximum[1].min(ceiling)];
    if minimum[0] > maximum[0] || minimum[1] > maximum[1] {
        return None;
    }
    Some(Size::new(minimum[0], minimum[1], maximum[0], maximum[1]))
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
    }
    Description::new(output, &formats, properties)
}

/// Describe the profile's allocation choices within the fixed KMS object envelope.
///
/// Geometry is intersected with native allocation limits, including the cursor limit.
/// Unsupported roles and formats contribute no allocations. An empty intersection fails;
/// it does not become unrestricted. The caller supplies standard scalar property rules.
/// Native publication still checks object membership, and the retained profile must validate
/// complete scenes, layout details, provenance, layer counts and color operations at acceptance.
/// This operation allocates metadata only; it neither establishes readiness nor grants access.
pub(crate) fn renderer(
    profile: &Profile,
    planes: &[Plane],
    properties: &[Property],
) -> Result<ARef<Description>> {
    check_planes(planes)?;
    let limits = profile.limits();
    let output = bounds(
        limits.geometry.min_output,
        limits.geometry.output,
        potential::MAX_DIMENSION,
    )
    .ok_or(EOPNOTSUPP)?;
    let mut formats = KVec::new();
    for plane in planes {
        let (role, ceiling) = match plane.kind {
            Kind::Primary => (0, potential::MAX_DIMENSION),
            Kind::Overlay => (1, potential::MAX_DIMENSION),
            Kind::Cursor => (2, potential::MAX_CURSOR_DIMENSION),
        };
        if limits.roles[role] == 0 {
            continue;
        }
        let Some(size) = bounds(limits.geometry.min_source, limits.geometry.source, ceiling) else {
            continue;
        };
        for format in profile.formats() {
            if !potential::FORMATS.contains(&format.fourcc)
                || (plane.kind == Kind::Cursor && format.fourcc != fourcc::ARGB8888)
                || format.planes as usize != crate::formats::plane_count(format.fourcc)
            {
                continue;
            }
            let format = match format.modifier {
                Some(modifier) => Format::new(plane.id, format.fourcc, modifier, size),
                None => Format::implicit(plane.id, format.fourcc, size),
            };
            formats.push(format, GFP_KERNEL)?;
        }
    }
    if formats.is_empty() {
        return Err(EOPNOTSUPP);
    }
    Description::new(output, &formats, properties)
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
            },
            Plane {
                id: 8,
                kind: Kind::Overlay,
            },
            Plane {
                id: 9,
                kind: Kind::Cursor,
            },
        ]
    }

    #[test]
    fn exact_output_retains_tiled_and_implicit_source_choices() -> Result {
        let profile = profile(limits())?;
        let rules = [Property::unsigned_range(8, 17, 1, 3)];
        let description = renderer(&profile, &planes(), &rules)?;
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
        assert_eq!(description.properties()[0].bounds(), (1, 3));
        Ok(())
    }

    #[test]
    fn unsupported_roles_do_not_advertise_allocations() -> Result {
        let mut limits = limits();
        limits.roles = [1, 0, 0];
        let description = renderer(&profile(limits)?, &planes(), &[])?;
        assert_eq!(description.formats().len(), 3);
        assert!(description
            .formats()
            .iter()
            .all(|format| format.plane_id() == 7));
        limits.roles = [0, 0, 1];
        limits.geometry.min_source = [1024; 2];
        assert!(matches!(
            renderer(&profile(limits)?, &planes(), &[]),
            Err(EOPNOTSUPP)
        ));
        Ok(())
    }

    #[test]
    fn allocation_intersection_never_widens_exact_geometry() -> Result {
        let mut limits = limits();
        limits.geometry.output = [32768; 2];
        let description = renderer(&profile(limits)?, &planes(), &[])?;
        assert_eq!(description.output().maximum(), (16384, 16384));
        limits.geometry.min_output = [32768; 2];
        assert!(matches!(
            renderer(&profile(limits)?, &planes(), &[]),
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
        let description = renderer(&profile, &planes()[..1], &[])?;
        assert_eq!(description.output().minimum(), (5120, 2880));
        assert_eq!(description.output().maximum(), (5120, 2880));
        assert_eq!(description.formats().len(), 1);
        assert_eq!(description.formats()[0].format(), fourcc::XRGB16161616F);
        assert_eq!(description.formats()[0].modifier(), Some(TILED));
        Ok(())
    }

    #[test]
    fn ambiguous_plane_identity_is_rejected() -> Result {
        let profile = profile(limits())?;
        let mut planes = planes();
        assert!(matches!(renderer(&profile, &[], &[]), Err(EINVAL)));
        planes[1].id = planes[0].id;
        assert!(matches!(renderer(&profile, &planes, &[]), Err(EINVAL)));
        planes[1].id = 0;
        assert!(matches!(renderer(&profile, &planes, &[]), Err(EINVAL)));
        Ok(())
    }

    #[test]
    fn host_describes_only_builtin_allocation_choices() -> Result {
        let rules = [Property::unsigned_range(8, 17, 1, 30)];
        let description = host(&planes(), &rules)?;
        assert_eq!(description.output().minimum(), (1, 1));
        assert_eq!(description.output().maximum(), (8192, 8192));
        assert_eq!(description.properties()[0].bounds(), (1, 30));
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
