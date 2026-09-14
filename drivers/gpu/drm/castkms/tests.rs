// SPDX-License-Identifier: GPL-2.0-only

//! Real atomic callbacks with synthetic master observations; no userspace authority claims.

use super::*;

/// Returns through the test's Rust cleanup before KUnit reports a failure.
#[track_caller]
fn check(condition: bool) -> Result {
    if condition {
        Ok(())
    } else {
        let location = core::panic::Location::caller();
        pr_err!(
            "CastKMS check failed at {}:{}\n",
            location.file(),
            location.line()
        );
        Err(EINVAL)
    }
}

mod capture_configurations;
#[cfg(CONFIG_DRM_CLIENT)]
mod capture_control;
#[cfg(CONFIG_DRM_CLIENT)]
mod capture_descriptions;
#[cfg(CONFIG_DRM_CLIENT)]
mod capture_destinations;
#[cfg(CONFIG_DRM_CLIENT)]
mod capture_files;
#[cfg(CONFIG_DRM_CLIENT)]
mod capture_host_stream;
#[cfg(CONFIG_DRM_CLIENT)]
mod capture_permissions;
#[cfg(CONFIG_DRM_CLIENT)]
mod capture_snapshots;
#[cfg(CONFIG_DRM_CLIENT)]
mod capture_provider;
mod capture_streams;
mod blank_ownership;
mod host_blank;
mod configurations;
#[cfg(CONFIG_DRM_CLIENT)]
mod display_control;
mod generations;
mod gem_budget;
mod host_attempts;
mod host_cache;
mod host_composition;
mod host_configuration;
mod host_delivery;
mod host_framebuffers;
mod host_validation;
mod host_idle;
mod host_images;
mod host_intervals;
mod host_layouts;
mod host_origins;
mod host_pool;
mod host_requests;
mod host_results;
mod host_snapshots;
mod host_wait;
mod host_worker;
#[cfg(CONFIG_DRM_CLIENT)]
mod image_access;
mod output_identity;
#[cfg(CONFIG_DRM_CLIENT)]
mod imports;
mod producers;
mod renderer_startup;
#[cfg(CONFIG_DRM_CLIENT)]
mod renderer_permission;
#[cfg(CONFIG_DRM_CLIENT)]
mod renderer_candidates;
#[cfg(CONFIG_DRM_CLIENT)]
mod renderer_snapshots;
mod vblank;
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
    host_budget: kernel::sync::Arc<host_compositor::budget::Budget>,
    drm: TestDevice<Driver>,
    _parent: faux::Registration,
}

/// Keeps the exporter's parent bound until the callback's deferred file releases end.
#[cfg(CONFIG_DRM_CLIENT)]
fn with_exporter(test: impl FnOnce(&Fixture) -> Result) -> Result {
    let source = Fixture::new()?;
    let result = test(&source);
    // SAFETY: Called by the import tests in a kernel thread, with no locks held.
    // Their callbacks release every exported buffer and imported object before returning.
    unsafe { kernel::bindings::flush_delayed_fput() };
    result
}

impl Fixture {
    fn new() -> Result<Self> {
        Self::new_named(c"castkms-attribution-test")
    }

    fn new_named(name: &'static kernel::str::CStr) -> Result<Self> {
        let parent = faux::Registration::new_with_dma_mask(
            name,
            None,
            kernel::dma::DmaMask::new::<64>(),
        )?;
        let state = device::Owner::new()?;
        let drm =
            drm::UnregisteredDevice::<Driver>::new(parent.as_ref(), Ok::<_, Error>(state.state()))?;
        let drm = TestDevice::new(drm)?;
        Ok(Self {
            state,
            host_budget: host_compositor::budget::Budget::new()?,
            drm,
            _parent: parent,
        })
    }

    fn framebuffer(&self, data: provenance::Provenance) -> Result<FramebufferRef<Driver>> {
        let object = shmem::Object::<gem::Object>::new(
            self.drm.device(),
            640 * 480 * 4,
            Default::default(),
            Default::default(),
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
        check(fixture.has_owner(Some(&owner)))?;
        let b = fixture.drm.synthetic_master_snapshot(true)?;
        fixture
            .drm
            .device()
            .authority
            .changed(Some(b.master().clone()));
        fixture.select(&fb, false, 0)?;
        check(fixture.has_owner(Some(&owner)))?;
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
        check(fixture.has_owner(Some(&owner)))?;
        check(fixture.select(&replacement, false, 1).is_err())?;
        check(fixture.has_owner(Some(&owner)))?;
        fixture.select(&replacement, false, 0)?;
        check(fixture.has_owner(Some(&replacement_owner)))?;
        Ok(())
    }

    #[test]
    fn unknown_same_framebuffer_does_not_adopt_new_master() -> Result {
        let fixture = Fixture::new()?;
        let fb = fixture.framebuffer(provenance::Provenance::from_snapshot(None))?;
        fixture.select(&fb, false, 0)?;
        check(fixture.has_owner(None))?;
        let master = fixture.drm.synthetic_master_snapshot(true)?;
        fixture
            .drm
            .device()
            .authority
            .changed(Some(master.master().clone()));
        fixture.select(&fb, false, 0)?;
        check(fixture.has_owner(None))?;
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
        check(fixture.has_owner(Some(&owner)))?;
        fixture.state.close();
        fixture.select(&fb, false, 0)?;
        check(fixture.drm.device().output.inspect(|scene| scene.is_none()))?;
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
        check(fixture.has_owner(Some(&owner)))?;
        let crtc = fixture.drm.crtc()?;
        fixture
            .drm
            .update(|transaction| transaction.set_crtc_config(crtc, None))?;
        check(fixture.drm.device().output.inspect(|scene| scene.is_none()))?;
        fixture.select(&fb, false, 0)?;
        check(fixture.has_owner(Some(&owner)))?;
        Ok(())
    }
}
