// SPDX-License-Identifier: GPL-2.0-only

//! Retained color policy and integer arithmetic for CPU composition.

mod tables;

use kernel::{
    drm::kms::{
        colorop::Operation,
        crtc::{ColorCtm, ColorLut},
    },
    prelude::*,
    sync::Arc,
};

/// Output-wide degamma, matrix and gamma, applied after plane composition.
pub(crate) struct OutputColor {
    degamma: Option<Arc<Gamma>>,
    matrix: Option<[u64; 12]>,
    gamma: Option<Arc<Gamma>>,
}

impl OutputColor {
    pub(crate) fn new(
        degamma: Option<&[ColorLut]>,
        ctm: Option<&ColorCtm>,
        gamma: Option<&[ColorLut]>,
    ) -> Result<Option<Arc<Self>>> {
        if degamma.is_none() && ctm.is_none() && gamma.is_none() {
            return Ok(None);
        }
        let matrix = ctm.map(|ctm| {
            core::array::from_fn(|index| {
                if index % 4 == 3 {
                    0
                } else {
                    ctm.raw()[(index / 4) * 3 + index % 4]
                }
            })
        });
        Ok(Some(Arc::new(
            Self {
                degamma: Gamma::new(degamma)?,
                matrix,
                gamma: Gamma::new(gamma)?,
            },
            GFP_KERNEL,
        )?))
    }

    pub(crate) fn apply(&self, mut channels: [u32; 3]) -> [u32; 3] {
        if let Some(degamma) = &self.degamma {
            channels = degamma.apply(channels);
        }
        if let Some(coefficients) = &self.matrix {
            channels = matrix(coefficients, channels.map(|value| value as i32))
                .map(|value| value.clamp(0, 65535) as u32);
        }
        if let Some(gamma) = &self.gamma {
            channels = gamma.apply(channels);
        }
        channels
    }
}

pub(crate) struct Pipeline {
    operations: KVec<Operation>,
}

impl Pipeline {
    pub(crate) fn new(operations: KVec<Operation>) -> Result<Option<Arc<Self>>> {
        if operations
            .iter()
            .all(|operation| matches!(operation, Operation::Bypass))
        {
            return Ok(None);
        }
        Ok(Some(Arc::new(Self { operations }, GFP_KERNEL)?))
    }

    /// Keep signed extended-range channels between matrices, clamping at curve boundaries.
    pub(crate) fn apply(&self, channels: [u32; 3]) -> [u32; 3] {
        let mut channels = channels.map(|value| value as i32);
        for operation in &self.operations {
            match operation {
                Operation::Bypass => {}
                Operation::SrgbEotf => {
                    channels = channels.map(|value| curve(&tables::SRGB_EOTF, value))
                }
                Operation::SrgbInverseEotf => {
                    channels = channels.map(|value| curve(&tables::SRGB_INVERSE_EOTF, value))
                }
                Operation::Matrix(coefficients) => channels = matrix(coefficients, channels),
            }
        }
        channels.map(|value| value.clamp(0, 65535) as u32)
    }
}

pub(crate) struct Gamma {
    entries: KVec<[u16; 3]>,
}

impl Gamma {
    pub(crate) fn new(entries: Option<&[ColorLut]>) -> Result<Option<Arc<Self>>> {
        let Some(entries) = entries else {
            return Ok(None);
        };
        if entries.is_empty() || entries.len() > 256 {
            return Err(EINVAL);
        }
        let mut owned = KVec::with_capacity(entries.len(), GFP_KERNEL)?;
        for entry in entries {
            owned.push([entry.red(), entry.green(), entry.blue()], GFP_KERNEL)?;
        }
        Ok(Some(Arc::new(Self { entries: owned }, GFP_KERNEL)?))
    }

    pub(crate) fn apply(&self, channels: [u32; 3]) -> [u32; 3] {
        core::array::from_fn(|channel| {
            lookup(self.entries.len(), channels[channel] as i32, |index| {
                self.entries[index][channel]
            }) as u32
        })
    }
}

fn lookup(length: usize, input: i32, entry: impl Fn(usize) -> u16) -> i32 {
    let position = input.clamp(0, 65535) as u64 * (length - 1) as u64;
    let index = (position / 65535) as usize;
    let fraction = position % 65535;
    let lower = u64::from(entry(index));
    let upper = u64::from(entry((index + 1).min(length - 1)));
    ((lower * (65535 - fraction) + upper * fraction + 32767) / 65535) as i32
}

fn curve(table: &[u16; 256], value: i32) -> i32 {
    lookup(table.len(), value, |index| table[index])
}

fn matrix(coefficients: &[u64; 12], channels: [i32; 3]) -> [i32; 3] {
    let signed = |raw: u64| {
        let magnitude = i128::from(raw & !(1 << 63));
        if raw >> 63 != 0 {
            -magnitude
        } else {
            magnitude
        }
    };
    core::array::from_fn(|row| {
        let mut sum = signed(coefficients[row * 4 + 3]);
        for channel in 0..3 {
            sum += signed(coefficients[row * 4 + channel]) * i128::from(channels[channel]);
        }
        // All S31.32 coefficients and signed 32-bit inputs fit the wide accumulator.
        // Saturate extended range instead of letting hostile coefficients wrap.
        ((sum + (1 << 31)) >> 32).clamp(i128::from(i32::MIN), i128::from(i32::MAX)) as i32
    })
}

#[cfg(CONFIG_DRM_CASTKMS_KUNIT_TEST)]
#[kunit_tests(rust_castkms_color)]
mod tests {
    use super::*;

    #[test]
    fn gamma_interpolates_and_single_entry_is_constant() -> Result {
        let gamma = Gamma::new(Some(&[
            ColorLut::new(0, 65535, 0),
            ColorLut::new(65535, 0, 0),
        ]))?
        .ok_or(EINVAL)?;
        assert_eq!(gamma.apply([32768; 3]), [32768, 32767, 0]);
        let constant = Gamma::new(Some(&[ColorLut::new(123, 456, 789)]))?.ok_or(EINVAL)?;
        assert_eq!(constant.apply([65535, 0, 32768]), [123, 456, 789]);
        Ok(())
    }

    #[test]
    fn matrices_preserve_negative_values_until_pipeline_end() {
        let mut coefficients = [0; 12];
        coefficients[0] = (1 << 63) | (1 << 32);
        coefficients[5] = 1 << 32;
        coefficients[10] = 1 << 32;
        let negative = matrix(&coefficients, [12345, 456, 789]);
        assert_eq!(negative, [-12345, 456, 789]);
        assert_eq!(matrix(&coefficients, negative), [12345, 456, 789]);
    }

    #[test]
    fn matrices_saturate_extreme_coefficients() {
        assert_eq!(matrix(&[u64::MAX; 12], [i32::MAX; 3]), [i32::MIN; 3]);
        assert_eq!(matrix(&[i64::MAX as u64; 12], [i32::MAX; 3]), [i32::MAX; 3]);
    }

    #[test]
    fn curves_clamp_and_preserve_endpoints() {
        for table in [&tables::SRGB_EOTF, &tables::SRGB_INVERSE_EOTF] {
            assert_eq!(curve(table, -1), 0);
            assert_eq!(curve(table, 65536), 65535);
        }
        assert!(curve(&tables::SRGB_EOTF, 32768) < 15000);
        assert!(curve(&tables::SRGB_INVERSE_EOTF, 32768) > 47000);
    }
}
