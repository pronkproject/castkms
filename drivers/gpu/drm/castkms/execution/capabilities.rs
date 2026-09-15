// SPDX-License-Identifier: GPL-2.0-only

//! Immutable whole-scene requirements, without renderer authority or CPU layout policy.

use kernel::{drm::fourcc, prelude::*};

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

#[cfg_attr(not(CONFIG_DRM_CASTKMS_KUNIT_TEST), expect(dead_code))]
impl Profile {
    pub(crate) fn new(limits: Limits, formats: KVec<Format>) -> Result<Self> {
        let geometry = limits.geometry;
        if geometry.output.contains(&0)
            || geometry.source.contains(&0)
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
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
#[kunit_tests(rust_castkms_capabilities)]
mod tests {
    use super::*;

    fn limits() -> Limits {
        Limits {
            geometry: GeometryLimits {
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
