// SPDX-License-Identifier: GPL-2.0-only

//! Retained color policy and integer arithmetic for CPU composition.

use kernel::{
    drm::kms::{
        crtc::{ColorLut},
    },
    prelude::*,
    sync::Arc,
};

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

}
