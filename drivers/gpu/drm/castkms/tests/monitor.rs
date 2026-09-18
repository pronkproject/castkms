// SPDX-License-Identifier: GPL-2.0-only

//! Virtual monitor publication and terminal shutdown.

use super::*;
use crate::monitor::Monitor;
use kernel::drm::kms::connector::{Edid, Status};

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

fn tiled_edid(identity: &[u8; 9], horizontal_location: u8) -> Result<Edid> {
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
    displayid[10] = horizontal_location << 4;
    displayid[12] = 0x7f;
    displayid[13] = 0x07;
    displayid[14] = 0x37;
    displayid[15] = 0x04;
    displayid[21..30].copy_from_slice(identity);
    displayid[30] = 0u8.wrapping_sub(
        displayid[1..30]
            .iter()
            .fold(0u8, |sum, byte| sum.wrapping_add(*byte)),
    );
    checksum(displayid);
    Edid::new(&bytes)
}

#[kunit_tests(rust_castkms_monitor)]
mod cases {
    use super::*;

    #[test]
    fn reservation_preserves_disconnection_until_publication() -> Result {
        let driver = CastKms::new(c"castkms-monitor-pending")?;
        let device = driver._display.registration_guard().ok_or(ENODEV)?;
        let pending = device.monitor.reserve(&device)?;
        check(device.monitor.status() == Status::Disconnected)?;
        check(matches!(device.monitor.reserve(&device), Err(EBUSY)))?;
        drop(pending);
        check(device.monitor.status() == Status::Disconnected)?;
        let pending = device.monitor.reserve(&device)?;
        let control = pending.publish()?;
        check(device.monitor.status() == Status::Disconnected)?;
        drop(control);
        let pending = device.monitor.reserve(&device)?;
        device.monitor.close();
        check(matches!(pending.publish(), Err(ENODEV)))?;
        check(device.monitor.status() == Status::Disconnected)
    }

    #[test]
    fn only_explicit_attachment_connects_the_monitor() -> Result {
        let driver = CastKms::new(c"castkms-monitor-control")?;
        let device = driver._display.registration_guard().ok_or(ENODEV)?;
        check(device.monitor.status() == Status::Disconnected)?;
        let control = device.monitor.acquire(&device)?;
        check(device.monitor.status() == Status::Disconnected)?;
        control.attach(None)?;
        check(device.monitor.status() == Status::Connected)?;
        control.detach()?;
        check(device.monitor.status() == Status::Disconnected)?;
        drop(control);
        check(device.monitor.status() == Status::Disconnected)
    }

    #[test]
    fn control_is_exclusive_and_shutdown_is_terminal() -> Result {
        let driver = CastKms::new(c"castkms-monitor-exclusion")?;
        let device = driver._display.registration_guard().ok_or(ENODEV)?;
        let other = Monitor::new()?;
        check(matches!(other.acquire(&device), Err(EINVAL)))?;
        let control = device.monitor.acquire(&device)?;
        check(matches!(device.monitor.acquire(&device), Err(EBUSY)))?;
        device.monitor.close();
        check(device.monitor.status() == Status::Disconnected)?;
        check(control.attach(None) == Err(ENODEV))?;
        drop(control);
        check(matches!(device.monitor.acquire(&device), Err(ENODEV)))
    }

    #[test]
    fn group_reservation_publishes_and_releases_every_member() -> Result {
        let driver = CastKms::new_outputs(c"castkms-monitor-group", 3)?;
        let device = driver._display.registration_guard().ok_or(ENODEV)?;
        let pending = Monitor::reserve_group(&device, &[0, 2])?;
        check(matches!(
            device.displays[0].monitor.reserve(&device),
            Err(EBUSY)
        ))?;
        check(matches!(
            device.displays[2].monitor.reserve(&device),
            Err(EBUSY)
        ))?;
        check(device.displays[0].monitor.status() == Status::Disconnected)?;
        check(device.displays[2].monitor.status() == Status::Disconnected)?;

        let control = pending.publish()?;
        let mut edids = KVec::new();
        edids.push(tiled_edid(b"CASTTILE0", 0)?, GFP_KERNEL)?;
        edids.push(tiled_edid(b"CASTTILE0", 1)?, GFP_KERNEL)?;
        let topology = control.attach(edids)?;
        check(topology.identity() == b"CASTTILE0")?;
        check(topology.dimensions() == [2, 1])?;
        check(topology.tile_size() == [1920, 1080])?;
        check(topology.member_location(0) == Some([0, 0]))?;
        check(topology.member_location(1) == Some([1, 0]))?;
        check(topology.member_location(2).is_none())?;
        check(device.displays[0].monitor.status() == Status::Connected)?;
        check(device.displays[1].monitor.status() == Status::Disconnected)?;
        check(device.displays[2].monitor.status() == Status::Connected)?;
        for index in [0, 2] {
            check(
                device.displays[index].monitor.cec.snapshot()?.flags
                    & kernel::uapi::DRM_CASTKMS_CEC_STATE_MONITOR_ATTACHED
                    == 0,
            )?;
            #[cfg(CONFIG_DRM_CASTKMS_AUDIO)]
            check(matches!(
                device.displays[index].monitor.audio(),
                Err(ENOTCONN)
            ))?;
        }

        control.detach()?;
        check(device.displays[0].monitor.status() == Status::Disconnected)?;
        check(device.displays[2].monitor.status() == Status::Disconnected)?;
        drop(control);
        let first = device.displays[0].monitor.acquire(&device)?;
        let last = device.displays[2].monitor.acquire(&device)?;
        drop(last);
        drop(first);
        Ok(())
    }

    #[test]
    fn group_rejects_incomplete_or_ambiguous_topology_before_publication() -> Result {
        let driver = CastKms::new_outputs(c"castkms-monitor-group-validation", 2)?;
        let device = driver._display.registration_guard().ok_or(ENODEV)?;
        let control = Monitor::reserve_group(&device, &[0, 1])?.publish()?;

        let mut incomplete = KVec::new();
        incomplete.push(tiled_edid(b"CASTTILE0", 0)?, GFP_KERNEL)?;
        check(matches!(control.attach(incomplete), Err(EINVAL)))?;

        let mut duplicate = KVec::new();
        duplicate.push(tiled_edid(b"CASTTILE0", 0)?, GFP_KERNEL)?;
        duplicate.push(tiled_edid(b"CASTTILE0", 0)?, GFP_KERNEL)?;
        check(matches!(control.attach(duplicate), Err(EINVAL)))?;

        let mut mismatched = KVec::new();
        mismatched.push(tiled_edid(b"CASTTILE0", 0)?, GFP_KERNEL)?;
        mismatched.push(tiled_edid(b"CASTTILE1", 1)?, GFP_KERNEL)?;
        check(matches!(control.attach(mismatched), Err(EINVAL)))?;
        check(device.displays[0].monitor.status() == Status::Disconnected)?;
        check(device.displays[1].monitor.status() == Status::Disconnected)
    }

    #[test]
    fn failed_group_reservation_releases_earlier_members() -> Result {
        let driver = CastKms::new_outputs(c"castkms-monitor-group-unwind", 3)?;
        let device = driver._display.registration_guard().ok_or(ENODEV)?;
        let occupied = device.displays[1].monitor.acquire(&device)?;
        check(matches!(
            Monitor::reserve_group(&device, &[0, 1, 2]),
            Err(EBUSY)
        ))?;
        let first = device.displays[0].monitor.acquire(&device)?;
        let last = device.displays[2].monitor.acquire(&device)?;
        drop(last);
        drop(first);
        drop(occupied);
        check(matches!(
            Monitor::reserve_group(&device, &[1, 0]),
            Err(EINVAL)
        ))
    }
}
