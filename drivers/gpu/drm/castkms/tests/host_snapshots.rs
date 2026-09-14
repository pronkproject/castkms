// SPDX-License-Identifier: GPL-2.0-only

//! Startup copies preserve image origin without retaining a host slot or source claim.

use super::*;
use crate::{
    host_compositor::{
        compose,
        layout::Layout,
        pool::Pool, //
    },
    host_snapshot::{
        Budget,
        Snapshot, //
    }, //
};
use kernel::{
    drm::preparation::Source,
    io::{
        io_project,
        Io, //
    },
    sync::aref::ARef, //
};

fn display(fixture: &Fixture, width: u16, height: u16) -> Result {
    let layout = Layout::new(u32::from(width), u32::from(height))?;
    let object = shmem::Object::<gem::Object>::new(
        fixture.drm.device(),
        layout.size(),
        Default::default(),
        Default::default(),
    )?;
    let fb = fixture.drm.framebuffer(
        &FramebufferLayout {
            width: u32::from(width),
            height: u32::from(height),
            format: drm::fourcc::XRGB8888,
            modifier: Some(drm::fourcc::FORMAT_MOD_LINEAR),
            interlaced: false,
            planes: &[FramebufferPlane {
                object: &object,
                pitch: width as u32 * 4,
                offset: 0,
            }],
        },
        provenance::Provenance::from_snapshot(None),
    )?;
    let mode = DisplayMode::from_timings(ModeTimings {
        clock_khz: (((u32::from(width) + 24) * (u32::from(height) + 3) * 60).div_ceil(1000)) as i32,
        hdisplay: width,
        hsync_start: width + 8,
        hsync_end: width + 16,
        htotal: width + 24,
        vdisplay: height,
        vsync_start: height + 1,
        vsync_end: height + 2,
        vtotal: height + 3,
        flags: ModeFlags::NHSYNC | ModeFlags::NVSYNC,
    })?;
    let scanout = CrtcScanout {
        mode: &mode,
        framebuffer: &fb,
        connectors: &[fixture.drm.connector()?],
        position: (0, 0),
    };
    fixture.drm.update(|mut transaction| {
        transaction
            .as_mut()
            .set_crtc_config(fixture.drm.crtc()?, Some(&scanout))
    })
}

#[kunit_tests(rust_castkms_host_snapshots)]
mod cases {
    use super::*;

    #[test]
    fn snapshot_keeps_pixels_and_origin_without_retaining_a_host_slot() -> Result {
        let fixture = Fixture::new()?;
        let owner = fixture.drm.synthetic_master_snapshot(true)?;
        let master = owner.master().clone();
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(Some(owner)))?;
        {
            let map = fb.vmap::<gem::Object>()?;
            io_project!(map.view(), [try: 0..2560]).copy_from_slice(&[0x35; 2560]);
        }
        fixture.select(&fb, false, 0)?;
        let output = &fixture.drm.device().output;
        let source: ARef<Source> = output
            .inspect_accepted(|accepted| accepted.map(|(source, _)| source.into()))
            .ok_or(EINVAL)?;
        let layout = Layout::new(640, 480)?;
        let pool = Pool::new(fixture.drm.device(), &fixture.host_budget, layout)?;
        let image = compose::current(output, &pool)?.ok_or(EINVAL)?;
        let serial = image.content_serial();
        let completed_at = image.completed_at();
        let configuration = image.configuration().cloned();
        let budget = Budget::new()?;
        let snapshot = Snapshot::new(fixture.drm.device(), &budget, &image)?;
        drop(image);
        let first = pool.reserve()?;
        let second = pool.reserve()?;
        check(source.hold_admission()?.prepared()?.is_some())?;
        drop(first);
        drop(second);
        {
            let map = fb.vmap::<gem::Object>()?;
            io_project!(map.view(), [try: 0..2560]).copy_from_slice(&[0x71; 2560]);
        }
        fixture.select(&fb, false, 0)?;
        let replacement = compose::current(output, &pool)?.ok_or(EINVAL)?;
        check(replacement.content_serial() != serial)?;
        check(snapshot.content_serial() == serial)?;
        check(snapshot.completed_at() - completed_at == kernel::time::Delta::ZERO)?;
        check(snapshot.configuration() == configuration.as_ref())?;
        check(snapshot.output_identity() == output.identity())?;
        check(snapshot.owner() == Some(&master))?;
        check(snapshot.layout() == layout)?;
        drop(replacement);
        pool.close();
        fixture.state.close();
        drop(fb);
        let mut pixels = KVVec::new();
        pixels.resize(layout.pixel_bytes(), 0xff, GFP_KERNEL)?;
        snapshot.copy_pixels(&mut pixels)?;
        check(pixels[..2560] == [0x35; 2560])?;
        check(pixels[2560..].iter().all(|byte| *byte == 0))?;
        check(source.prepared()?.is_some())?;
        Ok(())
    }

    #[test]
    fn exhausted_snapshot_storage_does_not_consume_host_capacity() -> Result {
        let fixture = Fixture::new()?;
        display(&fixture, 1920, 1080)?;
        let output = &fixture.drm.device().output;
        let pool = Pool::new(
            fixture.drm.device(),
            &fixture.host_budget,
            Layout::new(1920, 1080)?,
        )?;
        let budget = Budget::new()?;
        let image = compose::current(output, &pool)?.ok_or(EINVAL)?;
        let first = Snapshot::new(fixture.drm.device(), &budget, &image)?;
        let _second = Snapshot::new(fixture.drm.device(), &budget, &image)?;
        check(matches!(
            Snapshot::new(fixture.drm.device(), &budget, &image),
            Err(EBUSY)
        ))?;
        drop(image);
        let image = compose::current(output, &pool)?.ok_or(EINVAL)?;
        check(matches!(
            Snapshot::new(fixture.drm.device(), &budget, &image),
            Err(EBUSY)
        ))?;
        drop(first);
        let _replacement = Snapshot::new(fixture.drm.device(), &budget, &image)?;
        Ok(())
    }

    #[test]
    fn blank_snapshot_has_initialized_padding_and_no_visible_content_serial() -> Result {
        let fixture = Fixture::new()?;
        display(&fixture, 17, 3)?;
        fixture
            .drm
            .update(|transaction| transaction.disable_plane(fixture.drm.plane()?))?;
        let layout = Layout::new(17, 3)?;
        let pool = Pool::new(fixture.drm.device(), &fixture.host_budget, layout)?;
        let image = compose::current(&fixture.drm.device().output, &pool)?.ok_or(EINVAL)?;
        let snapshot = Snapshot::new(fixture.drm.device(), &Budget::new()?, &image)?;
        let mut bytes = KVVec::new();
        bytes.resize(layout.size(), 0xff, GFP_KERNEL)?;
        check(layout.pixel_bytes() < layout.size())?;
        snapshot.copy_allocation_for_test(&mut bytes)?;
        check(bytes.iter().all(|byte| *byte == 0))?;
        check(snapshot.content_serial().is_none())?;
        check(snapshot.configuration().is_some())?;
        let mut wrong = [0x72; 16];
        check(snapshot.copy_pixels(&mut wrong) == Err(EINVAL))?;
        check(wrong == [0x72; 16])?;
        Ok(())
    }
}
