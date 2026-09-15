// SPDX-License-Identifier: GPL-2.0-only

//! Bounded complete-scene encoding and transactional descriptor publication.

use kernel::{drm::kms::colorop::Operation, prelude::*, uapi};

const MAX_BYTES: usize = uapi::DRM_CASTKMS_RENDERER_SCENE_MAX_BYTES as usize;

const _: () = {
    assert!(
        48 + super::description::MAX_LAYERS
            * (144 + super::description::MAX_COLOR_OPERATIONS * 104)
            + 2 * (8 + 256 * 8)
            + 104
            <= MAX_BYTES
    );
    assert!(super::description::MAX_LAYERS == uapi::DRM_CASTKMS_RENDERER_SCENE_MAX_LAYERS as usize);
    assert!(
        super::description::MAX_COLOR_OPERATIONS
            == uapi::DRM_CASTKMS_RENDERER_SCENE_MAX_COLOR_OPS as usize
    );
};

struct Encoding {
    bytes: KVec<u8>,
}

impl Encoding {
    fn new() -> Result<Self> {
        Ok(Self {
            bytes: KVec::with_capacity(MAX_BYTES, GFP_KERNEL)?,
        })
    }

    fn append(&mut self, bytes: &[u8]) -> Result {
        if bytes.len() > MAX_BYTES - self.bytes.len() {
            return Err(E2BIG);
        }
        for byte in bytes {
            self.bytes.push(*byte, GFP_KERNEL)?;
        }
        Ok(())
    }

    fn word(&mut self, value: u32) -> Result {
        self.append(&value.to_ne_bytes())
    }
    fn wide(&mut self, value: u64) -> Result {
        self.append(&value.to_ne_bytes())
    }
    fn patch(&mut self, offset: usize, value: usize) -> Result {
        let value = u32::try_from(value).map_err(|_| EOVERFLOW)?;
        let end = offset.checked_add(4).ok_or(EINVAL)?;
        self.bytes
            .get_mut(offset..end)
            .ok_or(EINVAL)?
            .copy_from_slice(&value.to_ne_bytes());
        Ok(())
    }

    fn matrix(&mut self, matrix: &[u64; 12]) -> Result {
        self.word(uapi::DRM_CASTKMS_RENDERER_COLOR_MATRIX)?;
        self.word(96)?;
        for value in matrix {
            self.wide(*value)?;
        }
        Ok(())
    }

    fn operation(&mut self, operation: &Operation) -> Result {
        let kind = match operation {
            Operation::Bypass => uapi::DRM_CASTKMS_RENDERER_COLOR_BYPASS,
            Operation::SrgbEotf => uapi::DRM_CASTKMS_RENDERER_COLOR_SRGB_EOTF,
            Operation::SrgbInverseEotf => uapi::DRM_CASTKMS_RENDERER_COLOR_SRGB_INVERSE_EOTF,
            Operation::Matrix(matrix) => return self.matrix(matrix),
        };
        self.word(kind)?;
        self.word(0)
    }

    fn lut(&mut self, entries: &[[u16; 3]]) -> Result {
        if entries.is_empty() || entries.len() > 256 {
            return Err(E2BIG);
        }
        self.word(uapi::DRM_CASTKMS_RENDERER_COLOR_LUT)?;
        self.word((entries.len() * 8) as u32)?;
        for entry in entries {
            for channel in entry {
                self.append(&channel.to_ne_bytes())?;
            }
            self.append(&0u16.to_ne_bytes())?;
        }
        Ok(())
    }
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
#[kunit_tests(rust_castkms_scene_encoding)]
mod tests {
    use super::*;

    #[test]
    fn color_records_preserve_coefficients_and_lut_channels() -> Result {
        let mut encoded = Encoding::new()?;
        let mut matrix = [0; 12];
        matrix[0] = (1 << 63) | (1 << 32);
        matrix[11] = 17;
        encoded.matrix(&matrix)?;
        assert_eq!(encoded.bytes.len(), 104);
        assert_eq!(&encoded.bytes[8..16], &matrix[0].to_ne_bytes());
        assert_eq!(&encoded.bytes[96..104], &17u64.to_ne_bytes());
        encoded.lut(&[[1, 2, 65535]])?;
        assert_eq!(encoded.bytes.len(), 120);
        assert_eq!(&encoded.bytes[112..114], &1u16.to_ne_bytes());
        assert_eq!(&encoded.bytes[116..118], &65535u16.to_ne_bytes());
        assert_eq!(&encoded.bytes[118..120], &[0, 0]);
        encoded.operation(&Operation::SrgbInverseEotf)?;
        assert_eq!(encoded.bytes.len(), 128);
        assert_eq!(encoded.lut(&[]), Err(E2BIG));
        Ok(())
    }

    #[test]
    fn metadata_capacity_is_a_hard_bound() -> Result {
        let mut encoded = Encoding::new()?;
        for _ in 0..MAX_BYTES / 8 {
            encoded.wide(0)?;
        }
        assert_eq!(encoded.word(1), Err(E2BIG));
        assert_eq!(encoded.bytes.len(), MAX_BYTES);
        assert_eq!(encoded.patch(MAX_BYTES, 0), Err(EINVAL));
        assert_eq!(encoded.patch(usize::MAX, 0), Err(EINVAL));
        Ok(())
    }
}
