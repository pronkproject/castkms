// SPDX-License-Identifier: GPL-2.0-only

//! Bounded capability encoding, independent of userspace access and authorization.

use crate::execution::{
    capabilities::{ColorLimits, Format, GeometryLimits, Limits, Profile},
    validation::Contract,
};
use kernel::{
    prelude::*,
    transmute::{AsBytes, FromBytes},
    uapi,
};

pub(super) const MAX_BYTES: usize = uapi::DRM_CASTKMS_CAPABILITY_MAX_BYTES as usize;

#[derive(Clone, Copy, Default)]
#[repr(C)]
struct Header {
    version: u32,
    kind: u32,
    flags: u32,
    format_count: u32,
    max_output: [u32; 2],
    max_source: [u32; 2],
    min_scale: u32,
    max_scale: u32,
    max_layers: u32,
    max_roles: [u32; 3],
    max_color_operations: u32,
    max_lut_entries: u32,
    yuv_encodings: u32,
    yuv_ranges: u32,
    min_output: [u32; 2],
    min_source: [u32; 2],
    reserved: [u32; 10],
}

// SAFETY: The header is entirely u32 fields, without padding or invalid bit patterns.
unsafe impl FromBytes for Header {}
// SAFETY: Every byte belongs to an initialized integer field.
unsafe impl AsBytes for Header {}

#[derive(Clone, Copy)]
#[repr(C)]
struct Storage {
    fourcc: u32,
    plane_count: u32,
    modifier: u64,
    flags: u32,
    pitch_alignment: u32,
    offset_alignment: u32,
    max_pitch: u32,
}

// SAFETY: All fields are integers, and the u64 is aligned without interior or tail padding.
unsafe impl FromBytes for Storage {}
// SAFETY: Every byte belongs to an initialized integer field.
unsafe impl AsBytes for Storage {}

const _: () = {
    assert!(core::mem::size_of::<Header>() == 128);
    assert!(
        core::mem::size_of::<Header>()
            == core::mem::size_of::<uapi::drm_castkms_capability_profile>()
    );
    assert!(core::mem::size_of::<Storage>() == 32);
    assert!(
        core::mem::size_of::<Storage>()
            == core::mem::size_of::<uapi::drm_castkms_capability_format>()
    );
    assert!(
        crate::execution::capabilities::MAX_FORMATS
            == uapi::DRM_CASTKMS_CAPABILITY_MAX_FORMATS as usize
    );
};

const FEATURES: u32 = uapi::DRM_CASTKMS_CAPABILITY_PROFILE_CROP
    | uapi::DRM_CASTKMS_CAPABILITY_PROFILE_FRACTIONAL
    | uapi::DRM_CASTKMS_CAPABILITY_PROFILE_POSITION
    | uapi::DRM_CASTKMS_CAPABILITY_PROFILE_SCALE
    | uapi::DRM_CASTKMS_CAPABILITY_PROFILE_SRGB
    | uapi::DRM_CASTKMS_CAPABILITY_PROFILE_PLANE_MATRIX
    | uapi::DRM_CASTKMS_CAPABILITY_PROFILE_OUTPUT_MATRIX;
const STORAGE: u32 = uapi::DRM_CASTKMS_CAPABILITY_FORMAT_NATIVE
    | uapi::DRM_CASTKMS_CAPABILITY_FORMAT_IMPORTED
    | uapi::DRM_CASTKMS_CAPABILITY_FORMAT_EXPLICIT_MODIFIER;
const YUV_ENCODINGS: u32 = uapi::DRM_CASTKMS_CAPABILITY_YUV_ENCODING_BT601
    | uapi::DRM_CASTKMS_CAPABILITY_YUV_ENCODING_BT709
    | uapi::DRM_CASTKMS_CAPABILITY_YUV_ENCODING_BT2020;
const YUV_RANGES: u32 = uapi::DRM_CASTKMS_CAPABILITY_YUV_RANGE_LIMITED
    | uapi::DRM_CASTKMS_CAPABILITY_YUV_RANGE_FULL;

/// None denotes the fixed HOST contract; renderer profiles remain owned values.
pub(super) fn decode(bytes: &[u8]) -> Result<Option<Profile>> {
    if bytes.len() > MAX_BYTES {
        return Err(E2BIG);
    }
    let (header, formats) = Header::from_bytes_copy_prefix(bytes).ok_or(EINVAL)?;
    if header.version != uapi::DRM_CASTKMS_CAPABILITY_VERSION {
        return Err(EOPNOTSUPP);
    }
    if header.reserved != [0; 10]
        || header.flags & !FEATURES != 0
        || header.yuv_encodings & !YUV_ENCODINGS != 0
        || header.yuv_ranges & !YUV_RANGES != 0
        || header.format_count as usize > crate::execution::capabilities::MAX_FORMATS
        || formats.len() != header.format_count as usize * core::mem::size_of::<Storage>()
    {
        return Err(EINVAL);
    }
    if header.kind == uapi::DRM_CASTKMS_CAPABILITY_KIND_HOST {
        if bytes[8..].iter().any(|byte| *byte != 0) {
            return Err(EINVAL);
        }
        return Ok(None);
    }
    if header.kind != uapi::DRM_CASTKMS_CAPABILITY_KIND_RENDERER {
        return Err(EINVAL);
    }
    let mut tuples = KVec::with_capacity(header.format_count as usize, GFP_KERNEL)?;
    for bytes in formats.chunks_exact(core::mem::size_of::<Storage>()) {
        let format = Storage::from_bytes_copy(bytes).ok_or(EINVAL)?;
        if format.flags & !STORAGE != 0
            || (format.flags & uapi::DRM_CASTKMS_CAPABILITY_FORMAT_EXPLICIT_MODIFIER == 0
                && format.modifier != 0)
        {
            return Err(EINVAL);
        }
        tuples.push(
            Format {
                fourcc: format.fourcc,
                modifier: (format.flags & uapi::DRM_CASTKMS_CAPABILITY_FORMAT_EXPLICIT_MODIFIER != 0)
                    .then_some(format.modifier),
                planes: format.plane_count,
                native: format.flags & uapi::DRM_CASTKMS_CAPABILITY_FORMAT_NATIVE != 0,
                imported: format.flags & uapi::DRM_CASTKMS_CAPABILITY_FORMAT_IMPORTED != 0,
                pitch_alignment: format.pitch_alignment,
                offset_alignment: format.offset_alignment,
                max_pitch: format.max_pitch,
            },
            GFP_KERNEL,
        )?;
    }
    let has = |flag| header.flags & flag != 0;
    Profile::new(
        Limits {
            geometry: GeometryLimits {
                min_output: header.min_output,
                min_source: header.min_source,
                output: header.max_output,
                source: header.max_source,
                crop: has(uapi::DRM_CASTKMS_CAPABILITY_PROFILE_CROP),
                fractional: has(uapi::DRM_CASTKMS_CAPABILITY_PROFILE_FRACTIONAL),
                position: has(uapi::DRM_CASTKMS_CAPABILITY_PROFILE_POSITION),
                scale: has(uapi::DRM_CASTKMS_CAPABILITY_PROFILE_SCALE),
                min_scale: header.min_scale,
                max_scale: header.max_scale,
            },
            color: ColorLimits {
                operations: header.max_color_operations as usize,
                srgb: has(uapi::DRM_CASTKMS_CAPABILITY_PROFILE_SRGB),
                plane_matrix: has(uapi::DRM_CASTKMS_CAPABILITY_PROFILE_PLANE_MATRIX),
                output_matrix: has(uapi::DRM_CASTKMS_CAPABILITY_PROFILE_OUTPUT_MATRIX),
                lut_entries: header.max_lut_entries as usize,
                yuv_encodings: core::array::from_fn(|bit| header.yuv_encodings & (1 << bit) != 0),
                yuv_ranges: core::array::from_fn(|bit| header.yuv_ranges & (1 << bit) != 0),
            },
            layers: header.max_layers as usize,
            roles: header.max_roles.map(|count| count as usize),
        },
        tuples,
    )
    .map(Some)
}

pub(super) fn encode(contract: &Contract) -> Result<KVec<u8>> {
    let mut header = Header {
        version: uapi::DRM_CASTKMS_CAPABILITY_VERSION,
        kind: uapi::DRM_CASTKMS_CAPABILITY_KIND_HOST,
        ..Default::default()
    };
    let mut bytes = KVec::with_capacity(MAX_BYTES, GFP_KERNEL)?;
    if let Contract::Renderer(profile) = contract {
        let limits = profile.limits();
        let geometry = limits.geometry;
        let color = limits.color;
        header.kind = uapi::DRM_CASTKMS_CAPABILITY_KIND_RENDERER;
        header.format_count = profile.formats().len() as u32;
        for (enabled, flag) in [
            (geometry.crop, uapi::DRM_CASTKMS_CAPABILITY_PROFILE_CROP),
            (geometry.fractional, uapi::DRM_CASTKMS_CAPABILITY_PROFILE_FRACTIONAL),
            (geometry.position, uapi::DRM_CASTKMS_CAPABILITY_PROFILE_POSITION),
            (geometry.scale, uapi::DRM_CASTKMS_CAPABILITY_PROFILE_SCALE),
            (color.srgb, uapi::DRM_CASTKMS_CAPABILITY_PROFILE_SRGB),
            (
                color.plane_matrix,
                uapi::DRM_CASTKMS_CAPABILITY_PROFILE_PLANE_MATRIX,
            ),
            (
                color.output_matrix,
                uapi::DRM_CASTKMS_CAPABILITY_PROFILE_OUTPUT_MATRIX,
            ),
        ] {
            if enabled {
                header.flags |= flag;
            }
        }
        header.max_output = geometry.output;
        header.max_source = geometry.source;
        header.min_output = geometry.min_output;
        header.min_source = geometry.min_source;
        header.min_scale = geometry.min_scale;
        header.max_scale = geometry.max_scale;
        header.max_layers = limits.layers as u32;
        header.max_roles = limits.roles.map(|count| count as u32);
        header.max_color_operations = color.operations as u32;
        header.max_lut_entries = color.lut_entries as u32;
        for (bit, enabled) in color.yuv_encodings.into_iter().enumerate() {
            if enabled {
                header.yuv_encodings |= 1 << bit;
            }
        }
        for (bit, enabled) in color.yuv_ranges.into_iter().enumerate() {
            if enabled {
                header.yuv_ranges |= 1 << bit;
            }
        }
    }
    bytes.extend_from_slice(header.as_bytes(), GFP_KERNEL)?;
    if let Contract::Renderer(profile) = contract {
        for format in profile.formats() {
            let storage = Storage {
                fourcc: format.fourcc,
                plane_count: format.planes,
                modifier: format.modifier.unwrap_or(0),
                flags: u32::from(format.native) * uapi::DRM_CASTKMS_CAPABILITY_FORMAT_NATIVE
                    | u32::from(format.imported) * uapi::DRM_CASTKMS_CAPABILITY_FORMAT_IMPORTED
                    | u32::from(format.modifier.is_some())
                        * uapi::DRM_CASTKMS_CAPABILITY_FORMAT_EXPLICIT_MODIFIER,
                pitch_alignment: format.pitch_alignment,
                offset_alignment: format.offset_alignment,
                max_pitch: format.max_pitch,
            };
            bytes.extend_from_slice(storage.as_bytes(), GFP_KERNEL)?;
        }
    }
    Ok(bytes)
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
#[kunit_tests(rust_castkms_capability_encoding)]
mod tests {
    use super::*;
    use kernel::sync::Arc;

    fn profile() -> Result<Profile> {
        let mut formats = KVec::new();
        formats.push(
            Format {
                fourcc: kernel::drm::fourcc::XRGB8888,
                modifier: Some(0x0100_0000_0000_0001),
                planes: 1,
                native: false,
                imported: true,
                pitch_alignment: 128,
                offset_alignment: 4096,
                max_pitch: 65536,
            },
            GFP_KERNEL,
        )?;
        Profile::new(
            Limits {
                geometry: GeometryLimits {
                    min_output: [1920, 1080],
                    min_source: [64, 32],
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
            },
            formats,
        )
    }

    #[test]
    fn renderer_roundtrip_preserves_tiled_whole_scene_limits() -> Result {
        let profile = profile()?;
        let bytes = encode(&Contract::Renderer(Arc::new(profile, GFP_KERNEL)?))?;
        let decoded = decode(&bytes)?.ok_or(EINVAL)?;
        assert_eq!(decoded.limits().geometry.output, [16384; 2]);
        assert_eq!(decoded.limits().geometry.min_output, [1920, 1080]);
        assert_eq!(decoded.limits().geometry.min_source, [64, 32]);
        assert_eq!(decoded.formats()[0].modifier, Some(0x0100_0000_0000_0001));
        let again = encode(&Contract::Renderer(Arc::new(decoded, GFP_KERNEL)?))?;
        assert_eq!(&*bytes, &*again);
        Ok(())
    }

    #[test]
    fn host_is_a_canonical_fixed_contract() -> Result {
        let mut bytes = encode(&Contract::Host)?;
        assert_eq!(bytes.len(), 128);
        assert!(decode(&bytes)?.is_none());
        bytes[8] = 1;
        assert!(matches!(decode(&bytes), Err(EINVAL)));
        bytes[8] = 0;
        bytes[0..4].copy_from_slice(&1u32.to_ne_bytes());
        assert!(matches!(decode(&bytes), Err(EOPNOTSUPP)));
        Ok(())
    }

    #[test]
    fn parser_rejects_unknown_bits_reserved_words_and_partial_records() -> Result {
        let profile = profile()?;
        let original = encode(&Contract::Renderer(Arc::new(profile, GFP_KERNEL)?))?;
        for offset in [8, 64, 68, 72, 124, 144] {
            let mut bytes = KVec::new();
            bytes.extend_from_slice(&original, GFP_KERNEL)?;
            bytes[offset..offset + 4].copy_from_slice(&u32::MAX.to_ne_bytes());
            assert!(matches!(decode(&bytes), Err(EINVAL)));
        }
        for size in [0, 127, 128, 159] {
            assert!(matches!(decode(&original[..size]), Err(EINVAL)));
        }
        Ok(())
    }

    #[test]
    fn maximum_table_is_bounded_and_duplicate_tuples_are_rejected() -> Result {
        let reference = profile()?;
        let mut formats = KVec::new();
        for modifier in 0..crate::execution::capabilities::MAX_FORMATS {
            let mut format = reference.formats()[0];
            format.modifier = Some(modifier as u64);
            formats.push(format, GFP_KERNEL)?;
        }
        let profile = Profile::new(*reference.limits(), formats)?;
        let mut bytes = encode(&Contract::Renderer(Arc::new(profile, GFP_KERNEL)?))?;
        assert_eq!(bytes.len(), MAX_BYTES);
        assert_eq!(decode(&bytes)?.ok_or(EINVAL)?.formats().len(), 256);
        let first = Storage::from_bytes_copy(&bytes[128..160]).ok_or(EINVAL)?;
        bytes[160..192].copy_from_slice(first.as_bytes());
        assert!(matches!(decode(&bytes), Err(EINVAL)));
        bytes.push(0, GFP_KERNEL)?;
        assert!(matches!(decode(&bytes), Err(E2BIG)));
        Ok(())
    }
}
