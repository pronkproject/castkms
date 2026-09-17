// SPDX-License-Identifier: GPL-2.0-only

//! Renderer control on registered displays, with real master and atomic callbacks.

mod native_constraints;
mod offer_drafts;
mod offer_lifetimes;
mod offer_reaping;
mod offer_publication;
mod offer_sources;
mod endpoints;
mod endpoint_streams;
mod endpoint_withdrawal;
mod endpoint_outputs;
mod native_host_images;
mod issuer_lifetime;
mod multi_output_offers;
pub(super) mod private_images;

use super::*;
use crate::{
    capture::{permission as capture_permission, provider::Grantor},
    renderer::permission::{Owner, Permission},
};
use kernel::drm::{
    device::Registered,
    kms::{
        connector::Connector,
        crtc::Crtc,
        framebuffer::Framebuffer,
        testing::RegisteredMasterFile,
    },
    Device,
};

pub(super) fn with_display(
    f: impl FnOnce(
        &Device<Driver, Registered>,
        &Crtc<display::Crtc>,
        &Connector<display::Connector>,
        &CrtcScanout<'_, Driver>,
        RegisteredMasterFile<'_, Driver>,
    ) -> Result,
) -> Result {
    let display = CastKms::new(c"castkms-renderer-control")?;
    with_registered_display(&display, f)
}

fn with_registered_display(
    display: &CastKms,
    f: impl FnOnce(
        &Device<Driver, Registered>,
        &Crtc<display::Crtc>,
        &Connector<display::Connector>,
        &CrtcScanout<'_, Driver>,
        RegisteredMasterFile<'_, Driver>,
    ) -> Result,
) -> Result {
    let registered = display._display.registration_guard().ok_or(ENODEV)?;
    let file = RegisteredMasterFile::new(&registered)?;
    let crtc = file.crtc_at(0)?.to_owned_ref();
    let connector = file.connector_at(0)?;
    let object = shmem::Object::<gem::Object>::new(
        &registered, 640 * 480 * 4, Default::default(), Default::default(),
    )?;
    let framebuffer = Framebuffer::from_objects(&registered, &FramebufferLayout {
        width: 640, height: 480, format: drm::fourcc::XRGB8888, modifier: None,
        interlaced: false,
        planes: &[FramebufferPlane { object: &object, pitch: 2560, offset: 0 }],
    })?;
    let mode = DisplayMode::from_timings(ModeTimings {
        clock_khz: 25175, hdisplay: 640, hsync_start: 656, hsync_end: 752, htotal: 800,
        vdisplay: 480, vsync_start: 490, vsync_end: 492, vtotal: 525,
        flags: ModeFlags::NHSYNC | ModeFlags::NVSYNC,
    })?;
    let connectors = [&*connector];
    let scanout = CrtcScanout {
        mode: &mode,
        framebuffer: &framebuffer,
        connectors: &connectors,
        position: (0, 0),
    };
    registered.atomic_update(|state| state.set_crtc_config(crtc.crtc(), Some(&scanout)))?;
    f(&registered, crtc.crtc(), &connector, &scanout, file)
}

fn owner(file: &RegisteredMasterFile<'_, Driver>, crtc: &Crtc<display::Crtc>,
    connector: &Connector<display::Connector>) -> Result<Owner> {
    let permission = {
        let snapshot = file.file().master_snapshot().ok_or(EINVAL)?;
        let guard = snapshot.master().lock_current().ok_or(EACCES)?;
        Permission::new(&guard, crtc, connector)?
    };
    Owner::new(permission)
}

fn capture_grant(file: &RegisteredMasterFile<'_, Driver>, crtc: &Crtc<display::Crtc>,
    connector: &Connector<display::Connector>) -> Result<Grantor> {
    let snapshot = file.file().master_snapshot().ok_or(EINVAL)?;
    let permission = {
        let guard = snapshot.master().lock_current().ok_or(EACCES)?;
        capture_permission::Permission::new(&guard, crtc, connector)?
    };
    Grantor::new(permission)
}
