// SPDX-License-Identifier: GPL-2.0-only

//! Immutable whole-scene requirements, without renderer authority or CPU layout policy.

use crate::scene::{Geometry, Kind, Scene};
use kernel::{
    drm::{fourcc, gem::BaseObject, kms::colorop::Operation},
    prelude::*,
};

pub(crate) const MAX_FORMATS: usize = 256;

/// An exact storage tuple. An absent modifier is distinct from explicit linear.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Format {
    pub(crate) fourcc: u32,
    pub(crate) modifier: Option<u64>,
    pub(crate) planes: u32,
    pub(crate) native: bool,
    pub(crate) imported: bool,
    /// Byte alignment and maximum pitch, applied to every memory plane.
    pub(crate) pitch_alignment: u32,
    pub(crate) offset_alignment: u32,
    pub(crate) max_pitch: u32,
}

#[derive(Clone, Copy)]
pub(crate) struct GeometryLimits {
    /// Inclusive framebuffer and output dimension bounds, independently per axis.
    pub(crate) min_output: [u32; 2],
    pub(crate) min_source: [u32; 2],
    pub(crate) output: [u32; 2],
    pub(crate) source: [u32; 2],
    pub(crate) crop: bool,
    pub(crate) fractional: bool,
    pub(crate) position: bool,
    pub(crate) scale: bool,
    /// Source extent / destination extent, unsigned 16.16, inclusive.
    pub(crate) min_scale: u32,
    pub(crate) max_scale: u32,
}

#[derive(Clone, Copy)]
pub(crate) struct ColorLimits {
    pub(crate) operations: usize,
    pub(crate) srgb: bool,
    pub(crate) plane_matrix: bool,
    pub(crate) output_matrix: bool,
    pub(crate) lut_entries: usize,
    /// Supported YUV encodings/ranges, indexed like the complete-scene protocol.
    pub(crate) yuv_encodings: [bool; 3],
    pub(crate) yuv_ranges: [bool; 2],
}

#[derive(Clone, Copy)]
pub(crate) struct Limits {
    pub(crate) geometry: GeometryLimits,
    pub(crate) color: ColorLimits,
    pub(crate) layers: usize,
    /// Primary, overlay and cursor counts, respectively.
    pub(crate) roles: [usize; 3],
}

/// Construction validates and owns the description. No method mutates an accepted profile.
///
/// Matching proves declared compatibility only, not working GPU import or synchronization.
/// All tuples use the scene protocol's nearest-neighbor sampling, premultiplied
/// source-over blending, and stable zpos ordering. Limits apply to every role;
/// renderers with narrower per-role restrictions must advertise their intersection.
pub(crate) struct Profile {
    limits: Limits,
    formats: KVec<Format>,
}

impl Profile {
    pub(crate) fn new(limits: Limits, formats: KVec<Format>) -> Result<Self> {
        let geometry = limits.geometry;
        if geometry.output.contains(&0)
            || geometry.source.contains(&0)
            || geometry.min_output.contains(&0)
            || geometry.min_source.contains(&0)
            || (0..2).any(|axis| {
                geometry.min_output[axis] > geometry.output[axis]
                    || geometry.min_source[axis] > geometry.source[axis]
            })
            || geometry.min_scale == 0
            || geometry.min_scale > geometry.max_scale
            || limits.layers == 0
            || limits.layers > crate::scene::MAX_PLANES
            || limits.roles.iter().any(|count| *count > limits.layers)
            || limits.roles == [0; 3]
            || limits.color.operations > 16
            || limits.color.lut_entries > 256
            || formats.is_empty()
            || formats.len() > MAX_FORMATS
        {
            return Err(EINVAL);
        }
        for (index, format) in formats.iter().enumerate() {
            if format.fourcc == 0
                || format.modifier == Some(fourcc::FORMAT_MOD_INVALID)
                || !(1..=4).contains(&format.planes)
                || (!format.native && !format.imported)
                || !format.pitch_alignment.is_power_of_two()
                || !format.offset_alignment.is_power_of_two()
                || format.max_pitch < format.pitch_alignment
                // One set of layout constraints must cover the entire framebuffer.
                || formats[..index].iter().any(|previous| {
                    previous.fourcc == format.fourcc
                        && previous.modifier == format.modifier
                        && previous.planes == format.planes
                })
            {
                return Err(EINVAL);
            }
        }
        Ok(Self { limits, formats })
    }

    pub(crate) fn limits(&self) -> &Limits {
        &self.limits
    }
    pub(crate) fn formats(&self) -> &[Format] {
        &self.formats
    }

    fn storage(
        &self,
        fourcc: u32,
        modifier: Option<u64>,
        planes: usize,
        imported: bool,
        pitch: u32,
        offset: u32,
    ) -> bool {
        self.formats.iter().any(|format| {
            format.fourcc == fourcc
                && format.modifier == modifier
                && format.planes as usize == planes
                && pitch != 0
                && pitch <= format.max_pitch
                && pitch % format.pitch_alignment == 0
                && offset % format.offset_alignment == 0
                && if imported {
                    format.imported
                } else {
                    format.native
                }
        })
    }

    /// Validate every contributing layer and output operation without mapping storage.
    /// Empty scenes are allowed at an otherwise supported output size.
    pub(crate) fn check(&self, scene: &Scene, output: [u32; 2]) -> Result {
        self.check_output(output)?;
        let mut roles = [0; 3];
        let mut count = 0;
        for layer in scene.layers() {
            count += 1;
            let role = match layer.kind {
                Kind::Primary => 0,
                Kind::Overlay => 1,
                Kind::Cursor => 2,
            };
            roles[role] += 1;
            if count > self.limits.layers || roles[role] > self.limits.roles[role] {
                return Err(EOPNOTSUPP);
            }
            let image = layer.framebuffer();
            self.check_geometry(layer.geometry(), [image.width(), image.height()], output)?;
            for index in 0..image.plane_count() {
                let imported = image.object_at(index)?.imported_dma_buf().is_some();
                if !self.storage(
                    image.format(),
                    image.modifier(),
                    image.plane_count(),
                    imported,
                    image.pitch(index)?,
                    image.offset(index)?,
                ) {
                    return Err(EOPNOTSUPP);
                }
            }
            if image.plane_count() == 0 || image.is_interlaced() {
                return Err(EOPNOTSUPP);
            }
            self.check_color(layer.color.as_deref())?;
            use kernel::drm::kms::plane::{ColorEncoding, ColorRange};
            let encoding = match layer.yuv.0 {
                ColorEncoding::Bt601 => kernel::uapi::DRM_CASTKMS_YUV_ENCODING_BT601 as usize,
                ColorEncoding::Bt709 => kernel::uapi::DRM_CASTKMS_YUV_ENCODING_BT709 as usize,
                ColorEncoding::Bt2020 => kernel::uapi::DRM_CASTKMS_YUV_ENCODING_BT2020 as usize,
            };
            let range = match layer.yuv.1 {
                ColorRange::Limited => kernel::uapi::DRM_CASTKMS_YUV_RANGE_LIMITED as usize,
                ColorRange::Full => kernel::uapi::DRM_CASTKMS_YUV_RANGE_FULL as usize,
            };
            if image.is_yuv()
                && (!self.limits.color.yuv_encodings[encoding]
                    || !self.limits.color.yuv_ranges[range])
            {
                return Err(EOPNOTSUPP);
            }
        }
        if let Some(color) = &scene.output_color {
            let (degamma, matrix, gamma) = color.description();
            if (matrix.is_some() && !self.limits.color.output_matrix)
                || degamma.is_some_and(|lut| lut.len() > self.limits.color.lut_entries)
                || gamma.is_some_and(|lut| lut.len() > self.limits.color.lut_entries)
            {
                return Err(EOPNOTSUPP);
            }
        }
        Ok(())
    }

    pub(crate) fn check_output(&self, output: [u32; 2]) -> Result {
        if output.contains(&0)
            || (0..2).any(|axis| {
                output[axis] < self.limits.geometry.min_output[axis]
                    || output[axis] > self.limits.geometry.output[axis]
            })
        {
            return Err(EOPNOTSUPP);
        }
        Ok(())
    }

    fn check_geometry(&self, geometry: Geometry, source: [u32; 2], output: [u32; 2]) -> Result {
        let limits = self.limits.geometry;
        if geometry.output != output
            || source.contains(&0)
            || (0..2).any(|axis| {
                source[axis] < limits.min_source[axis] || source[axis] > limits.source[axis]
            })
            || (!limits.position && geometry.position != [0, 0])
            || (!limits.fractional && geometry.source.iter().any(|value| value & 0xffff != 0))
        {
            return Err(EOPNOTSUPP);
        }
        for axis in 0..2 {
            let origin = u64::from(geometry.source[axis]);
            let extent = u64::from(geometry.source[axis + 2]);
            let destination = u64::from(geometry.destination[axis]);
            let full = u64::from(source[axis]) << 16;
            if extent == 0
                || destination == 0
                || origin + extent > full
                || (!limits.crop && (origin != 0 || extent != full))
                || (!limits.scale && extent != destination << 16)
                || extent < destination * u64::from(limits.min_scale)
                || extent > destination * u64::from(limits.max_scale)
            {
                return Err(EOPNOTSUPP);
            }
        }
        Ok(())
    }

    fn check_color(&self, pipeline: Option<&crate::color::Pipeline>) -> Result {
        let Some(pipeline) = pipeline else {
            return Ok(());
        };
        let limits = self.limits.color;
        if pipeline.operations().len() > limits.operations {
            return Err(EOPNOTSUPP);
        }
        for operation in pipeline.operations() {
            match operation {
                Operation::Bypass => {}
                Operation::SrgbEotf | Operation::SrgbInverseEotf if limits.srgb => {}
                Operation::Matrix(_) if limits.plane_matrix => {}
                _ => return Err(EOPNOTSUPP),
            }
        }
        Ok(())
    }
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
#[kunit_tests(rust_castkms_capabilities)]
mod tests {
    use super::*;

    fn limits() -> Limits {
        Limits {
            geometry: GeometryLimits {
                min_output: [1; 2],
                min_source: [1; 2],
                output: [16384; 2],
                source: [16384; 2],
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
            layers: 24,
            roles: [1, 22, 1],
        }
    }

    fn profile(limits: Limits, modifier: Option<u64>) -> Result<Profile> {
        let mut formats = KVec::new();
        formats.push(
            Format {
                fourcc: fourcc::XRGB8888,
                modifier,
                planes: 1,
                native: false,
                imported: true,
                pitch_alignment: 4,
                offset_alignment: 4,
                max_pitch: 65536,
            },
            GFP_KERNEL,
        )?;
        Profile::new(limits, formats)
    }

    #[test]
    fn exact_dimensions_reject_smaller_and_larger_scenes() -> Result {
        let mut limits = limits();
        limits.geometry.min_output = [1920, 1080];
        limits.geometry.output = [1920, 1080];
        limits.geometry.min_source = [640, 480];
        limits.geometry.source = [640, 480];
        let profile = profile(limits, None)?;
        profile.check_output([1920, 1080])?;
        for output in [[1919, 1080], [1921, 1080], [1920, 1079], [1920, 1081]] {
            assert_eq!(profile.check_output(output), Err(EOPNOTSUPP));
        }
        // Source limits describe the complete framebuffer, not the cropped area.
        let geometry = Geometry {
            source: [0, 0, 320 << 16, 240 << 16],
            position: [0; 2],
            destination: [640, 480],
            output: [1920, 1080],
        };
        profile.check_geometry(geometry, [640, 480], geometry.output)?;
        for source in [[639, 480], [641, 480], [640, 479], [640, 481]] {
            assert_eq!(
                profile.check_geometry(geometry, source, geometry.output),
                Err(EOPNOTSUPP)
            );
        }
        Ok(())
    }

    #[test]
    fn dimension_ranges_are_positive_and_ordered() -> Result {
        for axis in 0..2 {
            for minimum in [0, 16385] {
                let mut bad = limits();
                bad.geometry.min_output[axis] = minimum;
                assert!(matches!(profile(bad, None), Err(EINVAL)));
                let mut bad = limits();
                bad.geometry.min_source[axis] = minimum;
                assert!(matches!(profile(bad, None), Err(EINVAL)));
            }
        }
        Ok(())
    }

    #[test]
    fn modifier_and_provenance_are_exact() -> Result {
        let modifier = Some(0x0100_0000_0000_0001);
        let profile = profile(limits(), modifier)?;
        assert!(profile.storage(fourcc::XRGB8888, modifier, 1, true, 16, 0));
        assert!(!profile.storage(fourcc::XRGB8888, modifier, 1, false, 16, 0));
        assert!(!profile.storage(fourcc::XRGB8888, Some(0), 1, true, 16, 0));
        assert!(!profile.storage(fourcc::XRGB8888, None, 1, true, 16, 0));
        assert!(!profile.storage(fourcc::XRGB8888, modifier, 2, true, 16, 0));
        assert!(!profile.storage(fourcc::XRGB8888, modifier, 1, true, 15, 0));
        assert!(!profile.storage(fourcc::XRGB8888, modifier, 1, true, 16, 1));
        assert!(!profile.storage(fourcc::XRGB8888, modifier, 1, true, 65540, 0));
        profile.check_output([16384; 2])?;
        assert_eq!(profile.check_output([16385, 1]), Err(EOPNOTSUPP));
        Ok(())
    }

    #[test]
    fn invalid_and_duplicate_descriptions_are_rejected() -> Result {
        assert!(matches!(
            profile(limits(), Some(fourcc::FORMAT_MOD_INVALID)),
            Err(EINVAL)
        ));
        let mut bad = limits();
        bad.geometry.min_scale = 0;
        assert!(matches!(profile(bad, None), Err(EINVAL)));
        let valid = profile(limits(), None)?;
        assert_eq!(valid.limits().layers, 24);
        let mut formats = KVec::new();
        formats.push(valid.formats()[0], GFP_KERNEL)?;
        let mut duplicate = valid.formats()[0];
        duplicate.native = true;
        duplicate.pitch_alignment = 8;
        formats.push(duplicate, GFP_KERNEL)?;
        assert!(matches!(Profile::new(limits(), formats), Err(EINVAL)));
        Ok(())
    }

    #[test]
    fn geometry_limits_do_not_round_scale_ratios() -> Result {
        let mut limits = limits();
        limits.geometry.min_scale = 1 << 16;
        limits.geometry.max_scale = 1 << 16;
        let profile = profile(limits, None)?;
        let mut geometry = Geometry {
            source: [0, 0, 2 << 16, 2 << 16],
            position: [-1, 0],
            destination: [2; 2],
            output: [2; 2],
        };
        profile.check_geometry(geometry, [2; 2], [2; 2])?;
        geometry.source[2] -= 1;
        assert_eq!(
            profile.check_geometry(geometry, [2; 2], [2; 2]),
            Err(EOPNOTSUPP)
        );
        geometry.source[0] = u32::MAX;
        assert_eq!(
            profile.check_geometry(geometry, [2; 2], [2; 2]),
            Err(EOPNOTSUPP)
        );
        Ok(())
    }

    #[test]
    fn geometry_features_are_explicit() -> Result {
        let limits = limits();
        let geometry = Geometry {
            source: [1 << 15, 0, 1 << 16, 2 << 16],
            position: [-1, 0],
            destination: [4; 2],
            output: [4; 2],
        };
        profile(limits, None)?.check_geometry(geometry, [2; 2], [4; 2])?;
        for geometry_limits in [
            GeometryLimits {
                crop: false,
                ..limits.geometry
            },
            GeometryLimits {
                fractional: false,
                ..limits.geometry
            },
            GeometryLimits {
                position: false,
                ..limits.geometry
            },
            GeometryLimits {
                scale: false,
                ..limits.geometry
            },
            GeometryLimits {
                source: [1; 2],
                ..limits.geometry
            },
        ] {
            assert_eq!(
                profile(
                    Limits {
                        geometry: geometry_limits,
                        ..limits
                    },
                    None
                )?
                .check_geometry(geometry, [2; 2], [4; 2]),
                Err(EOPNOTSUPP)
            );
        }
        Ok(())
    }

    #[test]
    fn plane_color_operations_require_declared_support() -> Result {
        let mut operations = KVec::new();
        operations.push(Operation::SrgbInverseEotf, GFP_KERNEL)?;
        operations.push(Operation::Matrix([0; 12]), GFP_KERNEL)?;
        let pipeline = crate::color::Pipeline::new(operations)?;
        let limits = limits();
        profile(limits, None)?.check_color(pipeline.as_deref())?;
        for color in [
            ColorLimits {
                srgb: false,
                ..limits.color
            },
            ColorLimits {
                plane_matrix: false,
                ..limits.color
            },
            ColorLimits {
                operations: 1,
                ..limits.color
            },
        ] {
            assert_eq!(
                profile(Limits { color, ..limits }, None)?.check_color(pipeline.as_deref()),
                Err(EOPNOTSUPP)
            );
        }
        Ok(())
    }

    #[test]
    fn format_descriptions_are_bounded() -> Result {
        let valid = profile(limits(), None)?;
        let mut formats = KVec::new();
        for modifier in 0..MAX_FORMATS {
            formats.push(
                Format {
                    modifier: Some(modifier as u64),
                    ..valid.formats()[0]
                },
                GFP_KERNEL,
            )?;
        }
        let valid = Profile::new(limits(), formats)?;
        let mut formats = KVec::new();
        for format in valid.formats() {
            formats.push(*format, GFP_KERNEL)?;
        }
        formats.push(
            Format {
                modifier: None,
                ..valid.formats()[0]
            },
            GFP_KERNEL,
        )?;
        assert!(matches!(Profile::new(limits(), formats), Err(EINVAL)));
        for format in [
            Format {
                pitch_alignment: 0,
                ..valid.formats()[0]
            },
            Format {
                offset_alignment: 3,
                ..valid.formats()[0]
            },
            Format {
                max_pitch: 1,
                ..valid.formats()[0]
            },
            Format {
                native: false,
                imported: false,
                ..valid.formats()[0]
            },
            Format {
                planes: 5,
                ..valid.formats()[0]
            },
        ] {
            let mut formats = KVec::new();
            formats.push(format, GFP_KERNEL)?;
            assert!(matches!(Profile::new(limits(), formats), Err(EINVAL)));
        }
        Ok(())
    }
}
