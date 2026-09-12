// SPDX-License-Identifier: GPL-2.0-only

//! Real atomic callbacks with synthetic master observations; no userspace authority claims.

use super::*;

mod generations;
#[cfg(CONFIG_DRM_CLIENT)]
mod imports;
mod producers;
use kernel::drm::{
    auth::MasterRef,
    gem::shmem,
    kms::{
        atomic::CrtcScanout,
        framebuffer::{
            FramebufferLayout,
            FramebufferPlane,
            FramebufferRef, //
        },
        modes::{
            DisplayMode,
            ModeFlags,
            ModeTimings, //
        },
        testing::TestDevice, //
    }, //
};

// Close device-owned references before atomic shutdown, then release the faux parent last.
struct Fixture {
    state: device::Owner,
    drm: TestDevice<Driver>,
    _parent: faux::Registration,
}

impl Fixture {
    fn new() -> Result<Self> {
        let parent = faux::Registration::new_with_dma_mask(
            c"castkms-attribution-test",
            None,
            kernel::dma::DmaMask::new::<64>(),
        )?;
        let state = device::Owner::new()?;
        let drm =
            drm::UnregisteredDevice::<Driver>::new(parent.as_ref(), Ok::<_, Error>(state.state()))?;
        let drm = TestDevice::new(drm)?;
        Ok(Self {
            state,
            drm,
            _parent: parent,
        })
    }

    fn framebuffer(&self, data: provenance::Provenance) -> Result<FramebufferRef<Driver>> {
        let object = shmem::Object::<gem::Object>::new(
            self.drm.device(),
            640 * 480 * 4,
            Default::default(),
            (),
        )?;
        self.drm.framebuffer(
            &FramebufferLayout {
                width: 640,
                height: 480,
                format: drm::fourcc::XRGB8888,
                modifier: None,
                interlaced: false,
                planes: &[FramebufferPlane {
                    object: &object,
                    pitch: 640 * 4,
                    offset: 0,
                }],
            },
            data,
        )
    }

    fn select(&self, fb: &FramebufferRef<Driver>, check_only: bool, x: u16) -> Result {
        self.select_with_producer(fb, check_only, x, None)
    }

    fn select_with_producer(
        &self,
        fb: &FramebufferRef<Driver>,
        check_only: bool,
        x: u16,
        producer: Option<&kernel::dma_fence::Fence>,
    ) -> Result {
        let mode = DisplayMode::from_timings(ModeTimings {
            clock_khz: 25175,
            hdisplay: 640,
            hsync_start: 656,
            hsync_end: 752,
            htotal: 800,
            vdisplay: 480,
            vsync_start: 490,
            vsync_end: 492,
            vtotal: 525,
            flags: ModeFlags::NHSYNC | ModeFlags::NVSYNC,
        })?;
        let scanout = CrtcScanout {
            mode: &mode,
            framebuffer: fb,
            connectors: &[self.drm.connector()?],
            position: (x, 0),
        };
        let crtc = self.drm.crtc()?;
        let update =
            |mut transaction: Pin<&mut kernel::drm::kms::atomic::AtomicStateComposer<Driver>>| {
                transaction.as_mut().set_crtc_config(crtc, Some(&scanout))?;
                transaction
                    .add_plane_state(self.drm.plane()?)?
                    .set_producer_fence(producer.map(|fence| fence.to_owned_ref()));
                Ok(())
            };
        if check_only {
            self.drm.check(update)
        } else {
            self.drm.update(update)
        }
    }

    fn has_owner(&self, expected: Option<&MasterRef<Driver>>) -> bool {
        self.drm
            .device()
            .output
            .inspect(|scene| scene.is_some_and(|scene| scene.owner() == expected))
    }
}

#[kunit_tests(rust_castkms_atomic_ownership)]
mod cases {
    use super::*;

    #[test]
    fn same_framebuffer_preserves_owner_after_master_change() -> Result {
        let fixture = Fixture::new()?;
        let a = fixture.drm.synthetic_master_snapshot(true)?;
        let owner = a.master().clone();
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(Some(a)))?;
        fixture.drm.device().authority.changed(Some(owner.clone()));
        fixture.select(&fb, false, 0)?;
        assert!(fixture.has_owner(Some(&owner)));
        let b = fixture.drm.synthetic_master_snapshot(true)?;
        fixture
            .drm
            .device()
            .authority
            .changed(Some(b.master().clone()));
        fixture.select(&fb, false, 0)?;
        assert!(fixture.has_owner(Some(&owner)));
        Ok(())
    }

    #[test]
    fn unaccepted_replacement_does_not_publish_new_owner() -> Result {
        let fixture = Fixture::new()?;
        let a = fixture.drm.synthetic_master_snapshot(true)?;
        let owner = a.master().clone();
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(Some(a)))?;
        fixture.drm.device().authority.changed(Some(owner.clone()));
        fixture.select(&fb, false, 0)?;
        let b = fixture.drm.synthetic_master_snapshot(true)?;
        let replacement_owner = b.master().clone();
        let replacement = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture
            .drm
            .device()
            .authority
            .changed(Some(replacement_owner.clone()));
        fixture.select(&replacement, true, 0)?;
        assert!(fixture.has_owner(Some(&owner)));
        assert!(fixture.select(&replacement, false, 1).is_err());
        assert!(fixture.has_owner(Some(&owner)));
        fixture.select(&replacement, false, 0)?;
        assert!(fixture.has_owner(Some(&replacement_owner)));
        Ok(())
    }

    #[test]
    fn unknown_same_framebuffer_does_not_adopt_new_master() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        assert!(fixture.has_owner(None));
        let master = fixture.drm.synthetic_master_snapshot(true)?;
        fixture
            .drm
            .device()
            .authority
            .changed(Some(master.master().clone()));
        fixture.select(&fb, false, 0)?;
        assert!(fixture.has_owner(None));
        Ok(())
    }

    #[test]
    fn terminal_close_prevents_atomic_tail_republication() -> Result {
        let fixture = Fixture::new()?;
        let a = fixture.drm.synthetic_master_snapshot(true)?;
        let owner = a.master().clone();
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(Some(a)))?;
        fixture.drm.device().authority.changed(Some(owner.clone()));
        fixture.select(&fb, false, 0)?;
        assert!(fixture.has_owner(Some(&owner)));
        fixture.state.close();
        fixture.select(&fb, false, 0)?;
        fixture
            .drm
            .device()
            .output
            .inspect(|scene| assert!(scene.is_none()));
        Ok(())
    }

    #[test]
    fn master_loss_preserves_attribution_until_disable() -> Result {
        let fixture = Fixture::new()?;
        let snapshot = fixture.drm.synthetic_master_snapshot(true)?;
        let owner = snapshot.master().clone();
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(Some(snapshot)))?;
        fixture.drm.device().authority.changed(Some(owner.clone()));
        fixture.select(&fb, false, 0)?;
        fixture.drm.device().authority.changed(None);
        fixture.select(&fb, false, 0)?;
        assert!(fixture.has_owner(Some(&owner)));
        let crtc = fixture.drm.crtc()?;
        fixture
            .drm
            .update(|transaction| transaction.set_crtc_config(crtc, None))?;
        assert!(fixture.drm.device().output.inspect(|scene| scene.is_none()));
        fixture.select(&fb, false, 0)?;
        assert!(fixture.has_owner(Some(&owner)));
        Ok(())
    }
}
