// SPDX-License-Identifier: GPL-2.0 OR MIT

//! Ownership and framing checks for display identification data.

use crate::{drm::kms::connector::Edid, prelude::*};

const EDID_1080P: [u8; 128] = [
    0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00, 0x31, 0xd8, 0x2a, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x21, 0x01, 0x03, 0x81, 0xa0, 0x5a, 0x78, 0x0a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x01,
    0x01, 0x01, 0x01, 0x01, 0x01, 0x01, 0x02, 0x3a, 0x80, 0x18, 0x71, 0x38, 0x2d, 0x40, 0x58, 0x2c,
    0x45, 0x00, 0x40, 0x84, 0x63, 0x00, 0x00, 0x1e, 0x00, 0x00, 0x00, 0xfc, 0x00, 0x54, 0x65, 0x73,
    0x74, 0x20, 0x45, 0x44, 0x49, 0x44, 0x0a, 0x20, 0x20, 0x20, 0x00, 0x00, 0x00, 0xfd, 0x00, 0x32,
    0x46, 0x1e, 0x46, 0x0f, 0x00, 0x0a, 0x20, 0x20, 0x20, 0x20, 0x20, 0x20, 0x00, 0x00, 0x00, 0x10,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xab,
];

fn checksum(block: &mut [u8]) {
    let last = block.len() - 1;
    block[last] = 0;
    block[last] = 0u8.wrapping_sub(block.iter().fold(0u8, |sum, byte| sum.wrapping_add(*byte)));
}

fn tiled_edid(location: u8) -> [u8; 256] {
    let mut bytes = [0; 256];
    bytes[..128].copy_from_slice(&EDID_1080P);
    bytes[126] = 1;
    checksum(&mut bytes[..128]);

    let displayid = &mut bytes[128..];
    displayid[0] = 0x70;
    displayid[1] = 0x13;
    displayid[2] = 25;
    displayid[5] = 0x12;
    displayid[7] = 22;
    displayid[8] = 0x80;
    displayid[9] = 0x10;
    displayid[10] = location << 4;
    displayid[12] = 0x7f;
    displayid[13] = 0x07;
    displayid[14] = 0x37;
    displayid[15] = 0x04;
    displayid[21..30].copy_from_slice(b"CASTTILE0");
    displayid[30] = 0u8.wrapping_sub(
        displayid[1..30]
            .iter()
            .fold(0u8, |sum, byte| sum.wrapping_add(*byte)),
    );
    checksum(displayid);
    bytes
}

#[kunit_tests(rust_drm_edid)]
mod cases {
    use super::*;

    #[test]
    fn accepts_complete_owned_data() -> Result {
        let mut bytes = EDID_1080P;
        let edid = Edid::new(&bytes)?;
        bytes.fill(0);
        drop(edid);
        Ok(())
    }

    #[test]
    fn rejects_invalid_framing() {
        assert_eq!(Edid::new(&[]).err(), Some(EINVAL));
        assert_eq!(Edid::new(&[0; 129]).err(), Some(EINVAL));

        let mut checksum = EDID_1080P;
        checksum[127] ^= 1;
        assert_eq!(Edid::new(&checksum).err(), Some(EINVAL));

        let mut missing_extension = EDID_1080P;
        missing_extension[126] = 1;
        missing_extension[127] = missing_extension[127].wrapping_sub(1);
        assert_eq!(Edid::new(&missing_extension).err(), Some(EINVAL));
    }

    #[test]
    fn decodes_optional_tiled_monitor_topology() -> Result {
        assert!(Edid::new(&EDID_1080P)?.tile()?.is_none());

        let left = Edid::new(&tiled_edid(0))?.tile()?.ok_or(EINVAL)?;
        let right = Edid::new(&tiled_edid(1))?.tile()?.ok_or(EINVAL)?;
        assert_eq!(left.topology_id(), b"CASTTILE0");
        assert_eq!(left.topology_id(), right.topology_id());
        assert_eq!(left.tile().horizontal_tiles(), 2);
        assert_eq!(left.tile().vertical_tiles(), 1);
        assert_eq!(left.tile().horizontal_location(), 0);
        assert_eq!(right.tile().horizontal_location(), 1);
        assert_eq!(left.tile().vertical_location(), 0);
        assert_eq!(left.tile().width(), 1920);
        assert_eq!(left.tile().height(), 1080);
        assert!(left.tile().is_single_monitor());
        Ok(())
    }

    #[test]
    fn rejects_out_of_grid_tiled_monitor_location() -> Result {
        let edid = Edid::new(&tiled_edid(2))?;
        assert_eq!(edid.tile(), Err(EINVAL));
        Ok(())
    }

    #[test]
    fn rejects_ambiguous_tiled_monitor_topology() -> Result {
        let mut bytes = tiled_edid(0);
        let displayid = &mut bytes[128..];
        displayid.copy_within(5..30, 30);
        displayid[2] = 50;
        displayid[55] = 0u8.wrapping_sub(
            displayid[1..55]
                .iter()
                .fold(0u8, |sum, byte| sum.wrapping_add(*byte)),
        );
        checksum(displayid);
        let edid = Edid::new(&bytes)?;
        assert_eq!(edid.tile(), Err(EINVAL));
        Ok(())
    }
}
