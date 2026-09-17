// SPDX-License-Identifier: GPL-2.0-only

//! Registered audio attachment, authority and timer lifetimes.

use super::*;
use kernel::drm::kms::connector::Edid;

fn edid() -> Result<Edid> {
    let mut bytes = [0; 256];
    bytes[..8].copy_from_slice(&[0, 255, 255, 255, 255, 255, 255, 0]);
    bytes[8..12].copy_from_slice(&[0x31, 0xd8, 42, 0]);
    bytes[18..25].copy_from_slice(&[1, 3, 0x80, 52, 29, 120, 0x0a]);
    bytes[38..54].fill(1);
    bytes[54..59].copy_from_slice(&[0, 0, 0, 0xfc, 0]);
    bytes[59..72].copy_from_slice(b"CastKMS Audio");
    bytes[126] = 1;
    bytes[128..140].copy_from_slice(&[2, 3, 12, 0x40, 0x23, 0x09, 0x07, 0x01, 0x83, 1, 0, 0]);
    for block in bytes.chunks_exact_mut(128) {
        block[127] = 0u8.wrapping_sub(
            block[..127]
                .iter()
                .fold(0u8, |sum, byte| sum.wrapping_add(*byte)),
        );
    }
    Edid::new(&bytes)
}

#[kunit_tests(rust_castkms_audio_lifetime)]
mod cases {
    use super::*;

    #[test]
    fn idle_audio_is_silence_and_revocation_discards_queued_frames() -> Result {
        renderer_control::with_display(|device, crtc, connector, _, file| {
            let monitor = device.monitor.acquire(device)?;
            monitor.attach(Some(edid()?))?;
            let owner = File::issue_audio_owner(file.file(), crtc, connector)?;
            let access = owner.access();
            let mut samples = [0xff; 1920];
            check(access.read(&mut samples, false)? == samples.len())?;
            check(samples.iter().all(|sample| *sample == 0))?;
            drop(owner);
            check(matches!(access.read(&mut samples, true), Err(EKEYREVOKED)))
        })
    }

    #[test]
    fn detach_does_not_allow_old_handles_to_follow_reattach() -> Result {
        renderer_control::with_display(|device, crtc, connector, _, file| {
            let monitor = device.monitor.acquire(device)?;
            monitor.attach(Some(edid()?))?;
            let owner = File::issue_audio_owner(file.file(), crtc, connector)?;
            let old = owner.access();
            monitor.detach()?;
            check(old.check().is_err())?;
            monitor.attach(Some(edid()?))?;
            check(old.check().is_err())?;
            let next = File::issue_audio_owner(file.file(), crtc, connector)?;
            next.access().check()
        })
    }

    #[test]
    fn capture_is_exclusive_but_revocation_allows_a_new_owner() -> Result {
        renderer_control::with_display(|device, crtc, connector, _, file| {
            let monitor = device.monitor.acquire(device)?;
            monitor.attach(Some(edid()?))?;
            let owner = File::issue_audio_owner(file.file(), crtc, connector)?;
            check(matches!(
                File::issue_audio_owner(file.file(), crtc, connector),
                Err(EBUSY)
            ))?;
            let old = owner.access();
            drop(owner);
            let next = File::issue_audio_owner(file.file(), crtc, connector)?;
            check(old.check() == Err(EKEYREVOKED))?;
            next.access().check()
        })
    }

    #[test]
    fn creator_close_revokes_retained_audio() -> Result {
        renderer_control::with_display(|device, crtc, connector, _, file| {
            let monitor = device.monitor.acquire(device)?;
            monitor.attach(Some(edid()?))?;
            let owner = File::issue_audio_owner(file.file(), crtc, connector)?;
            let access = owner.access();
            drop(file);
            check(access.check() == Err(EKEYREVOKED))
        })
    }

    #[test]
    fn a_sink_without_audio_does_not_create_an_audio_source() -> Result {
        let driver = CastKms::new(c"castkms-audio-no-eld")?;
        let device = driver._display.registration_guard().ok_or(ENODEV)?;
        let monitor = device.monitor.acquire(&device)?;
        monitor.attach(None)?;
        check(matches!(device.monitor.audio(), Err(ENOTCONN)))
    }

    #[test]
    fn video_disable_suspends_samples_without_revoking_audio_authority() -> Result {
        renderer_control::with_display(|device, crtc, connector, scanout, file| {
            let monitor = device.monitor.acquire(device)?;
            monitor.attach(Some(edid()?))?;
            let owner = File::issue_audio_owner(file.file(), crtc, connector)?;
            device.atomic_update(|state| state.set_crtc_config(crtc, None))?;
            let mut samples = [0xff; 1920];
            owner.access().check()?;
            check(owner.access().read(&mut samples, true) == Err(EAGAIN))?;
            device.atomic_update(|state| state.set_crtc_config(crtc, Some(scanout)))?;
            check(owner.access().read(&mut samples, false)? == samples.len())?;
            check(samples.iter().all(|sample| *sample == 0))
        })
    }

    #[test]
    fn same_master_reacquisition_reactivates_audio_with_a_fresh_queue() -> Result {
        renderer_control::with_display(|device, crtc, connector, _, file| {
            let monitor = device.monitor.acquire(device)?;
            monitor.attach(Some(edid()?))?;
            let owner = File::issue_audio_owner(file.file(), crtc, connector)?;
            let access = owner.access();
            let master = file.file().master_snapshot().ok_or(EINVAL)?;
            let mut samples = [0xff; 1920];
            check(access.read(&mut samples, false)? == samples.len())?;

            <Driver as kernel::drm::Driver>::master_changed(device, None);
            check(access.check() == Err(EAGAIN))?;
            check(access.read(&mut samples, true) == Err(EAGAIN))?;
            <Driver as kernel::drm::Driver>::master_changed(
                device,
                Some(master.master().clone()),
            );

            access.check()?;
            check(access.read(&mut samples, false)? == samples.len())?;
            check(samples.iter().all(|sample| *sample == 0))
        })
    }

    #[test]
    fn eight_audio_attachments_have_independent_lifetimes() -> Result {
        let driver = CastKms::new_features(c"castkms-eight-audio", 8, false, false, false)?;
        let device = driver._display.registration_guard().ok_or(ENODEV)?;
        let mut controls = KVec::new();
        for output in &device.displays {
            let control = output.monitor.acquire(&device)?;
            control.attach(Some(edid()?))?;
            controls.push(control, GFP_KERNEL)?;
        }
        controls[3].detach()?;
        for (index, output) in device.displays.iter().enumerate() {
            check(output.monitor.audio().is_ok() == (index != 3))?;
        }
        controls[3].attach(Some(edid()?))?;
        check(device.displays[3].monitor.audio().is_ok())
    }
}
