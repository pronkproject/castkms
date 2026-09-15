// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Independently owned ELD snapshots built by DRM's native EDID parser.

use super::Edid;
use crate::{
    bindings,
    error::to_result,
    prelude::*,
    str::CStr, //
};

/// An immutable EDID-Like Data audio-capability snapshot.
pub struct Eld {
    bytes: [u8; bindings::MAX_ELD_BYTES as usize],
    len: usize,
}

impl Edid {
    /// Build ELD without modifying a connector. `displayport` selects DP rather than HDMI.
    ///
    /// Returns `None` when the EDID advertises no audio. Basic audio without explicit
    /// descriptors is represented by CTA's mandatory stereo LPCM descriptor.
    pub fn eld(&self, displayport: bool) -> Result<Option<Eld>> {
        let mut eld = Eld {
            bytes: [0; bindings::MAX_ELD_BYTES as usize],
            len: 0,
        };
        // SAFETY: Self owns a validated immutable EDID; output is exclusive and sized.
        let len = unsafe {
            bindings::drm_edid_build_eld(
                self.as_ptr(),
                eld.bytes.as_mut_ptr(),
                eld.bytes.len(),
                displayport,
            )
        };
        to_result(len)?;
        eld.len = len as usize;
        Ok((len != 0).then_some(eld))
    }
}

impl Eld {
    /// Borrow precisely the initialized baseline ELD, including dword padding.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }

    /// Copy the monitor name to caller storage and return a terminated string.
    pub fn monitor_name<'a>(&self, output: &'a mut [u8; 17]) -> &'a CStr {
        let len = (self.bytes[4] & 31) as usize;
        let len = self.bytes[20..20 + len]
            .iter()
            .position(|&byte| byte == 0)
            .unwrap_or(len);
        output.fill(0);
        output[..len].copy_from_slice(&self.bytes[20..20 + len]);
        // SAFETY: DRM bounds monitor names to 13 bytes; the scan above excludes embedded
        // NULs and the cleared output provides the final NUL.
        unsafe { CStr::from_bytes_with_nul_unchecked(&output[..len + 1]) }
    }
}

#[cfg(CONFIG_KUNIT)]
#[crate::prelude::kunit_tests(rust_drm_edid_eld)]
mod tests {
    use super::*;

    fn parse(bytes: &mut [u8]) -> Result<Option<Eld>> {
        bytes[..8].copy_from_slice(&[0, 255, 255, 255, 255, 255, 255, 0]);
        bytes[18..20].copy_from_slice(&[1, 3]);
        bytes[126] = (bytes.len() / 128 - 1) as u8;
        for block in bytes.chunks_exact_mut(128) {
            block[127] = 0u8.wrapping_sub(
                block[..127]
                    .iter()
                    .fold(0u8, |sum, byte| sum.wrapping_add(*byte)),
            );
        }
        Edid::new(bytes)?.eld(false)
    }

    #[test]
    fn malformed_collections_and_basic_audio() -> Result {
        let mut bytes = [0; 256];
        bytes[128..133].copy_from_slice(&[2, 3, 5, 0, 0x23]);
        assert!(parse(&mut bytes)?.is_none());
        bytes[131] = 0x40;
        let eld = parse(&mut bytes)?.ok_or(EINVAL)?;
        assert_eq!(&eld.as_bytes()[20..23], &[9, 7, 1]);
        Ok(())
    }

    #[test]
    fn sad_count_is_bounded_across_collections() -> Result {
        let mut bytes = [0; 384];
        for block in bytes[128..].chunks_exact_mut(128) {
            block[..5].copy_from_slice(&[2, 3, 35, 0, 0x3e]);
            for sad in block[5..35].chunks_exact_mut(3) {
                sad.copy_from_slice(&[9, 7, 1]);
            }
        }
        let eld = parse(&mut bytes)?.ok_or(EINVAL)?;
        assert_eq!(eld.as_bytes()[5] >> 4, 15);
        assert_eq!(eld.as_bytes().len(), 68);
        let dp = Edid::new(&bytes)?.eld(true)?.ok_or(EINVAL)?;
        assert_eq!(dp.as_bytes()[5] & 0x0c, 4);
        Ok(())
    }

    #[test]
    fn vendor_audio_and_embedded_nul_monitor_name() -> Result {
        let mut bytes = [0; 256];
        bytes[54..59].copy_from_slice(&[0, 0, 0, 0xfc, 0]);
        bytes[59..67].copy_from_slice(b"Cast\0KMS");
        bytes[67] = b'\n';
        bytes[128..143]
            .copy_from_slice(&[2, 3, 15, 0, 0x23, 9, 7, 1, 0x66, 3, 0x0c, 0, 0, 0, 0x80]);
        let eld = parse(&mut bytes)?.ok_or(EINVAL)?;
        assert_eq!(eld.as_bytes()[5] & 2, 2);
        assert_eq!(eld.monitor_name(&mut [0; 17]).to_bytes(), b"Cast");
        Ok(())
    }

    #[test]
    fn displayid_cta_requires_its_own_checksum() -> Result {
        let mut bytes = [0; 256];
        bytes[128..140].copy_from_slice(&[0x70, 0x20, 7, 0, 0, 0x81, 0, 4, 0x23, 9, 7, 1]);
        assert!(parse(&mut bytes)?.is_none());
        bytes[140] = 0u8.wrapping_sub(
            bytes[129..140]
                .iter()
                .fold(0u8, |sum, byte| sum.wrapping_add(*byte)),
        );
        let eld = parse(&mut bytes)?.ok_or(EINVAL)?;
        assert_eq!(&eld.as_bytes()[20..23], &[9, 7, 1]);
        Ok(())
    }
}
